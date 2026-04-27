use serde_json::Value;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

const URL: &str = "http://localhost:8080";
const USER: &str = "admin";
const PASS: &str = "admin";

const SEED_DOMAIN: &str = "itest-cli.local";
const SEED_ADMIN_NAME: &str = "admin";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_stalwart-cli")
}

fn run_args(args: &[&str]) -> Output {
    Command::new(bin())
        .args(["--url", URL, "--user", USER, "--password", PASS])
        .args(args)
        .output()
        .expect("failed to spawn stalwart-cli")
}

fn run_with_stdin(args: &[&str], stdin_data: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .args(["--url", URL, "--user", USER, "--password", PASS])
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

fn server_reachable() -> bool {
    static REACHABLE: OnceLock<bool> = OnceLock::new();
    *REACHABLE.get_or_init(|| run_args(&["describe"]).status.success())
}

macro_rules! require_server {
    () => {
        if !server_reachable() {
            eprintln!("skipping: stalwart instance at {URL} not reachable");
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
        let _ = Command::new(bin())
            .args([
                "--url",
                URL,
                "--user",
                USER,
                "--password",
                PASS,
                "delete",
                "Domain",
                "--ids",
                &self.0,
            ])
            .output();
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

    let mut destroy_seen = false;
    let mut create_seen = false;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("each line valid JSON");
        let kind = v["@type"].as_str().expect("@type field");
        match kind {
            "destroy" => {
                destroy_seen = true;
                if let Some(value) = v.get("value")
                    && let Some(obj) = value.as_object()
                {
                    assert!(
                        !obj.contains_key("@type"),
                        "snapshot destroys must not carry an @type filter (regression of multi-variant fix): {line}"
                    );
                }
            }
            "create" => create_seen = true,
            _ => {}
        }
    }
    assert!(destroy_seen, "expected at least one destroy op");
    assert!(create_seen, "expected at least one create op");
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
        .args(["--url", URL, "--user", USER, "--password", PASS, "describe"])
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
            URL,
            "--user",
            "admin",
            "--password",
            "definitely-wrong-password",
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
