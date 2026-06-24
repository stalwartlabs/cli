/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

mod common;

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

use common::stalwart;
use common::stalwart::{ADMIN_PASSWORD as PASS, ADMIN_USER as USER};

const SEED_DOMAIN: &str = "itest-cli.local";
const SEED_ADMIN_NAME: &str = "admin";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_stalwart-cli")
}

fn server() -> &'static stalwart::Stalwart {
    stalwart::shared().expect("stalwart test container should be available")
}

fn run_args(args: &[&str]) -> Output {
    Command::new(bin())
        .args([
            "--url",
            server().base_url(),
            "--user",
            USER,
            "--password",
            PASS,
            "--insecure",
        ])
        .args(args)
        .output()
        .expect("failed to spawn stalwart-cli")
}

fn run_with_stdin(args: &[&str], stdin_data: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .args([
            "--url",
            server().base_url(),
            "--user",
            USER,
            "--password",
            PASS,
            "--insecure",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin_data)
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

macro_rules! require_server {
    () => {
        if stalwart::shared().is_none() {
            eprintln!("skipping: stalwart test container unavailable");
            return;
        }
    };
}

fn stdout_string(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is valid utf8")
}

fn stderr_string(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("stderr is valid utf8")
}

fn assert_ok(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx} failed (exit={:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout_string(out),
        stderr_string(out),
    );
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), now)
}

struct DomainCleanup(String);

impl Drop for DomainCleanup {
    fn drop(&mut self) {
        let _ = run_args(&["delete", "Domain", "--ids", &self.0]);
    }
}

fn state_lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn ndjson_field(out: &Output, key: &str) -> Vec<String> {
    stdout_string(out)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .and_then(|v| v.get(key).and_then(Value::as_str).map(String::from))
        })
        .collect()
}

fn delete_ids(object: &str, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let csv = ids.join(",");
    let _ = run_args(&["delete", object, "--ids", &csv]);
}

fn delete_id(object: &str, id: &str) {
    let _ = run_args(&["delete", object, "--ids", id]);
}

fn ids_for(object: &str) -> Vec<String> {
    let out = run_args(&["query", object, "--fields", "id", "--json"]);
    if !out.status.success() {
        return Vec::new();
    }
    ndjson_field(&out, "id")
}

fn ids_where(object: &str, filter: &str) -> Vec<String> {
    let out = run_args(&[
        "query", object, "--where", filter, "--fields", "id", "--json",
    ]);
    if !out.status.success() {
        return Vec::new();
    }
    ndjson_field(&out, "id")
}

fn create_capture_id(args: &[&str]) -> Option<String> {
    let out = run_args(args);
    if !out.status.success() {
        return None;
    }
    stdout_string(&out)
        .split_whitespace()
        .last()
        .map(|s| s.trim_end_matches(['\n', '\r']).to_string())
}

struct SeededAdmin {
    domain_id: String,
    account_id: String,
    _guard: MutexGuard<'static, ()>,
}

impl Drop for SeededAdmin {
    fn drop(&mut self) {
        delete_id("Account", &self.account_id);
        let dkim = ids_where("DkimSignature", &format!("domainId={}", self.domain_id));
        delete_ids("DkimSignature", &dkim);
        delete_id("Domain", &self.domain_id);
    }
}

fn seed_admin_account() -> SeededAdmin {
    let guard = state_lock();
    let existing = ids_for("Account");
    delete_ids("Account", &existing);

    let domain_id = ids_where("Domain", &format!("name={SEED_DOMAIN}"))
        .into_iter()
        .next()
        .or_else(|| {
            create_capture_id(&[
                "create",
                "Domain",
                "--field",
                &format!("name={SEED_DOMAIN}"),
            ])
        })
        .expect("seed domain");

    let account_id = create_capture_id(&[
        "create",
        "Account/User",
        "--field",
        &format!("name={SEED_ADMIN_NAME}"),
        "--field",
        &format!("domainId={domain_id}"),
    ])
    .expect("seed admin account");

    SeededAdmin {
        domain_id,
        account_id,
        _guard: guard,
    }
}

#[test]
fn describe_lists_objects() {
    require_server!();
    let out = run_args(&["describe"]);
    assert_ok(&out, "describe");
    let s = stdout_string(&out);
    assert!(s.contains("Account"), "expected Account in describe output");
    assert!(s.contains("Domain"), "expected Domain in describe output");
}

#[test]
fn describe_account_shows_variants_with_no_em_dash() {
    require_server!();
    let out = run_args(&["describe", "Account"]);
    assert_ok(&out, "describe Account");
    let s = stdout_string(&out);
    assert!(s.contains("Variants:"));
    assert!(
        s.contains("User: User account"),
        "User variant label missing or wrong separator"
    );
    assert!(s.contains("Group: Group account"));
    assert!(
        !s.contains('\u{2014}'),
        "describe output must not contain em dash"
    );
}

#[test]
fn query_json_is_ndjson_not_array() {
    require_server!();
    let _seed = seed_admin_account();

    let out = run_args(&["query", "Account", "--fields", "id,name", "--json"]);
    assert_ok(&out, "query --json");
    let s = stdout_string(&out);
    let trimmed = s.trim();
    assert!(
        !trimmed.starts_with('['),
        "output must not be a JSON array: {trimmed}"
    );
    let mut count = 0usize;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("each line must be valid JSON");
        assert!(v.get("id").is_some(), "each row must have an id: {line}");
        count += 1;
    }
    assert!(count >= 1, "expected at least one Account row (admin)");
}

#[test]
fn query_table_render_includes_header_and_admin_row() {
    require_server!();
    let _seed = seed_admin_account();

    let out = run_args(&["query", "Account", "--fields", "id,name"]);
    assert_ok(&out, "query Account (table)");
    let s = stdout_string(&out);
    let mut lines = s.lines();
    let header = lines.next().expect("header line").to_string();
    assert!(
        header.contains("id") && header.contains("Username"),
        "expected `id` and `Username` columns in header: {header}"
    );
    let body = s
        .lines()
        .skip(1)
        .find(|l| l.contains("admin"))
        .expect("admin row should appear");
    assert!(body.contains("admin"));
}

#[test]
fn get_admin_account_renders_human() {
    require_server!();
    let seed = seed_admin_account();

    let out = run_args(&["get", "Account", &seed.account_id]);
    assert_ok(&out, "get Account <id>");
    let s = stdout_string(&out);
    assert!(
        s.contains("admin"),
        "expected admin name in get output: {s}"
    );
}

#[test]
fn snapshot_tracer_emits_ndjson_with_no_at_type_filter() {
    require_server!();
    let out = run_args(&["snapshot", "Tracer"]);
    assert_ok(&out, "snapshot Tracer");
    let s = stdout_string(&out);
    let trimmed = s.trim();
    assert!(
        !trimmed.starts_with('['),
        "snapshot output must be NDJSON, not a JSON array"
    );

    let mut upsert_seen = false;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("each line valid JSON");
        let kind = v["@type"].as_str().expect("@type field");
        assert_ne!(
            kind, "destroy",
            "snapshot must not emit destroy ops anymore: {line}"
        );
        if kind == "upsert" {
            upsert_seen = true;
        }
    }
    assert!(upsert_seen, "expected at least one upsert op");
}

#[test]
fn snapshot_apply_round_trip_tracer() {
    require_server!();
    let snap = run_args(&["snapshot", "Tracer"]);
    assert_ok(&snap, "snapshot Tracer");
    let plan = snap.stdout;

    let applied = run_with_stdin(&["apply", "--stdin", "--dry-run"], &plan);
    assert_ok(&applied, "apply --dry-run round-trip");
}

#[test]
fn apply_rejects_legacy_json_array_format() {
    require_server!();
    let legacy = b"[{\"@type\":\"destroy\",\"object\":\"Tracer\"}]\n";
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], legacy);
    assert!(!out.status.success(), "apply must reject JSON-array form");
    let err = stderr_string(&out);
    assert!(err.contains("line 1"), "error must point at line 1: {err}");
}

#[test]
fn apply_skips_blank_lines_and_handles_trailing_newline() {
    require_server!();
    let plan = b"\n\n{\"@type\":\"destroy\",\"object\":\"Tracer\"}\n\n";
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan);
    assert_ok(&out, "apply tolerates blank lines");
    let s = stderr_string(&out);
    assert!(
        s.contains("1 destroy"),
        "plan summary missing in stderr: {s}"
    );
}

#[test]
fn create_update_get_delete_domain() {
    require_server!();
    let name = format!("itest-{}.example.com", unique_suffix());

    let out = run_args(&["create", "Domain", "--field", &format!("name={name}")]);
    assert_ok(&out, "create Domain");
    let id = stdout_string(&out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim()
        .trim_end_matches(['\n', '\r'])
        .to_string();
    let _cleanup = DomainCleanup(id.clone());

    let out = run_args(&[
        "update",
        "Domain",
        &id,
        "--field",
        "description=integration test",
    ]);
    assert_ok(&out, "update Domain");

    let out = run_args(&["get", "Domain", &id, "--json"]);
    assert_ok(&out, "get Domain --json");
    let s = stdout_string(&out);
    let v: Value = serde_json::from_str(s.trim()).expect("get --json must emit single line");
    assert_eq!(v["description"].as_str(), Some("integration test"));
    assert_eq!(v["name"].as_str(), Some(name.as_str()));
}

#[test]
fn create_duplicate_renders_set_error_and_exits_nonzero() {
    require_server!();
    let name = format!("dup-{}.example.com", unique_suffix());

    let out = run_args(&["create", "Domain", "--field", &format!("name={name}")]);
    assert_ok(&out, "create Domain");
    let id = stdout_string(&out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim_end_matches(['\n', '\r'])
        .to_string();

    let dup = run_args(&["create", "Domain", "--field", &format!("name={name}")]);
    assert!(
        !dup.status.success(),
        "creating a duplicate domain must fail with a non-zero exit"
    );
    let err = stderr_string(&dup);
    assert!(
        err.contains("primaryKeyViolation"),
        "expected the real SetError type to render: {err}"
    );
    assert!(
        err.contains("name"),
        "expected the offending property to render: {err}"
    );

    delete_ids(
        "DkimSignature",
        &ids_where("DkimSignature", &format!("domainId={id}")),
    );
    delete_id("Domain", &id);
}

#[test]
fn update_unknown_id_errors_clearly() {
    require_server!();
    let out = run_args(&[
        "update",
        "Domain",
        "definitely-not-a-real-id",
        "--field",
        "description=x",
    ]);
    assert!(
        !out.status.success(),
        "update of unknown id must fail; got success.\nstdout: {}\nstderr: {}",
        stdout_string(&out),
        stderr_string(&out)
    );
}

#[test]
fn query_pipe_to_head_returns_zero_no_broken_pipe_error() {
    require_server!();
    let mut cli = Command::new(bin())
        .args([
            "--url",
            server().base_url(),
            "--user",
            USER,
            "--password",
            PASS,
            "--insecure",
            "describe",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli");
    let cli_stdout = cli.stdout.take().expect("piped stdout");
    let head = Command::new("head")
        .args(["-n", "2"])
        .stdin(cli_stdout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn head");
    let head_out = head.wait_with_output().expect("wait head");
    let cli_status = cli.wait().expect("wait cli");
    assert!(head_out.status.success(), "head should succeed");
    assert!(
        cli_status.success(),
        "cli must exit 0 on broken pipe, got: {cli_status:?}"
    );
    let n_lines = String::from_utf8_lossy(&head_out.stdout).lines().count();
    assert!(n_lines >= 1, "expected at least one line through the pipe");
}

#[test]
fn auth_failure_exits_nonzero_and_prints_error() {
    require_server!();
    let out = Command::new(bin())
        .args([
            "--url",
            server().base_url(),
            "--user",
            "admin",
            "--password",
            "definitely-wrong-password",
            "--insecure",
            "describe",
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "wrong password must fail");
    let err = stderr_string(&out);
    assert!(
        err.contains("authentication") || err.contains("401"),
        "expected auth error message; got: {err}"
    );
}

struct Purge {
    object: &'static str,
    id: String,
}

impl Drop for Purge {
    fn drop(&mut self) {
        let _ = run_args(&["delete", self.object, "--ids", &self.id]);
    }
}

struct DomainTree(String);

impl Drop for DomainTree {
    fn drop(&mut self) {
        delete_ids(
            "DkimSignature",
            &ids_where("DkimSignature", &format!("domainId={}", self.0)),
        );
        delete_id("Domain", &self.0);
    }
}

fn create_id(args: &[&str]) -> String {
    create_capture_id(args).expect("create should return an id")
}

#[test]
fn query_where_equality_returns_only_the_matching_row() {
    require_server!();
    let suffix = unique_suffix();
    let a = format!("weqa-{suffix}.example.com");
    let b = format!("weqb-{suffix}.example.com");
    let ida = create_id(&["create", "Domain", "--field", &format!("name={a}")]);
    let idb = create_id(&["create", "Domain", "--field", &format!("name={b}")]);
    let _ta = DomainTree(ida);
    let _tb = DomainTree(idb);

    let out = run_args(&[
        "query",
        "Domain",
        "--where",
        &format!("name={a}"),
        "--fields",
        "id,name",
        "--json",
    ]);
    assert_ok(&out, "query --where equality");
    let names: Vec<String> = ndjson_field(&out, "name");
    assert_eq!(
        names,
        vec![a.clone()],
        "equality must return exactly the one matching domain"
    );
}

#[test]
fn query_empty_result_exits_cleanly_with_no_rows() {
    require_server!();
    let out = run_args(&[
        "query",
        "Domain",
        "--where",
        "name=definitely-absent-zzz.invalid",
        "--fields",
        "id,name",
        "--json",
    ]);
    assert_ok(&out, "query with no matches must still exit 0");
    assert!(
        stdout_string(&out).trim().is_empty(),
        "an empty result set must produce no rows"
    );
}

#[test]
fn query_where_comparison_on_string_field_is_rejected() {
    require_server!();
    let out = run_args(&["query", "Domain", "--where", "name>aaa", "--fields", "id"]);
    assert!(
        !out.status.success(),
        "a comparison operator on a string field must be rejected"
    );
    assert!(
        stderr_string(&out).contains("only allowed on number or datetime"),
        "expected the operator-type rejection message: {}",
        stderr_string(&out)
    );
}

#[test]
fn get_unknown_id_errors_and_field_projection_limits_output() {
    require_server!();
    let suffix = unique_suffix();
    let name = format!("proj-{suffix}.example.com");
    let id = create_id(&["create", "Domain", "--field", &format!("name={name}")]);
    let _tree = DomainTree(id.clone());
    assert_ok(
        &run_args(&["update", "Domain", &id, "--field", "description=set"]),
        "set a description",
    );

    let bad = run_args(&["get", "Domain", "definitely-not-a-real-id"]);
    assert!(!bad.status.success(), "get of an unknown id must fail");

    let out = run_args(&["get", "Domain", &id, "--fields", "name", "--json"]);
    assert_ok(&out, "get --fields name --json");
    let v: Value = serde_json::from_str(stdout_string(&out).trim()).expect("single json line");
    assert_eq!(v.get("name").and_then(Value::as_str), Some(name.as_str()));
    assert!(
        v.get("description").is_none(),
        "field projection must omit unrequested properties: {v}"
    );
}

#[test]
fn delete_stdin_json_array_and_mixed_success_failure() {
    require_server!();
    let suffix = unique_suffix();
    let r1 = create_id(&[
        "create",
        "MtaRoute/Local",
        "--field",
        &format!("name=delr1-{suffix}.test"),
    ]);
    let r2 = create_id(&[
        "create",
        "MtaRoute/Local",
        "--field",
        &format!("name=delr2-{suffix}.test"),
    ]);

    let array = format!("[\"{r1}\",\"{r2}\"]");
    let out = run_with_stdin(&["delete", "MtaRoute", "--stdin"], array.as_bytes());
    assert_ok(&out, "delete --stdin JSON array");
    assert!(
        ids_for("MtaRoute").iter().all(|id| id != &r1 && id != &r2),
        "both routes must be deleted via --stdin"
    );

    let r3 = create_id(&[
        "create",
        "MtaRoute/Local",
        "--field",
        &format!("name=delr3-{suffix}.test"),
    ]);
    let _g = Purge {
        object: "MtaRoute",
        id: r3.clone(),
    };
    let mixed = format!("{r3} bogus-missing-id");
    let out = run_with_stdin(&["delete", "MtaRoute", "--stdin"], mixed.as_bytes());
    assert!(
        !out.status.success(),
        "a mix of valid and invalid ids must exit non-zero"
    );
    let report = format!("{}{}", stdout_string(&out), stderr_string(&out));
    assert!(
        report.contains("1 deleted") && report.contains("1 failed"),
        "expected a mixed success/failure summary: {report}"
    );
}

#[test]
fn create_via_json_file_and_stdin_each_succeed() {
    require_server!();
    let suffix = unique_suffix();

    let jname = format!("srcjson-{suffix}.example.com");
    let jid = create_id(&[
        "create",
        "Domain",
        "--json",
        &format!("{{\"name\":\"{jname}\"}}"),
    ]);
    let _jt = DomainTree(jid);

    let fname = format!("srcfile-{suffix}.example.com");
    let path = std::env::temp_dir().join(format!("itest-create-{suffix}.json"));
    std::fs::write(&path, format!("{{\"name\":\"{fname}\"}}")).expect("write temp file");
    let fid = create_id(&["create", "Domain", "--file", path.to_str().unwrap()]);
    let _ft = DomainTree(fid);
    let _ = std::fs::remove_file(&path);

    let sname = format!("srcstdin-{suffix}.example.com");
    let out = run_with_stdin(
        &["create", "Domain", "--stdin"],
        format!("{{\"name\":\"{sname}\"}}").as_bytes(),
    );
    assert_ok(&out, "create --stdin");
    let sid = stdout_string(&out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim_end_matches(['\n', '\r'])
        .to_string();
    let _st = DomainTree(sid);

    for n in [&jname, &fname, &sname] {
        assert_eq!(
            ids_where("Domain", &format!("name={n}")).len(),
            1,
            "domain {n} must have been created"
        );
    }
}

#[test]
fn create_slash_form_sets_variant_at_type() {
    require_server!();
    let _guard = state_lock();
    let suffix = unique_suffix();
    let domain_id = create_id(&[
        "create",
        "Domain",
        "--field",
        &format!("name=slash-{suffix}.example.com"),
    ]);
    let _tree = DomainTree(domain_id.clone());
    let account_id = create_id(&[
        "create",
        "Account/User",
        "--field",
        &format!("name=slashu-{suffix}"),
        "--field",
        &format!("domainId={domain_id}"),
    ]);
    let _acc = Purge {
        object: "Account",
        id: account_id.clone(),
    };

    let out = run_args(&["get", "Account", &account_id, "--json"]);
    assert_ok(&out, "get account --json");
    let v: Value = serde_json::from_str(stdout_string(&out).trim()).expect("single json line");
    assert_eq!(
        v.get("@type").and_then(Value::as_str),
        Some("User"),
        "the slash form Account/User must create a User-variant object"
    );
}

#[test]
fn describe_enum_lists_its_variants() {
    require_server!();
    let out = run_args(&["describe", "MtaIpStrategy"]);
    assert_ok(&out, "describe <EnumName>");
    let s = stdout_string(&out);
    assert!(s.contains("(enum)"), "expected enum header: {s}");
    assert!(
        s.contains("v4ThenV6"),
        "expected enum variants to be listed: {s}"
    );
}
