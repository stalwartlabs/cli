/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

mod common;

use common::stalwart;
use common::stalwart::{ADMIN_PASSWORD as PASS, ADMIN_USER as USER};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_stalwart-cli")
}

fn server() -> &'static stalwart::Stalwart {
    stalwart::shared().expect("stalwart test container should be available")
}

macro_rules! require_server {
    () => {
        if stalwart::shared().is_none() {
            eprintln!("skipping: stalwart test container unavailable");
            return;
        }
    };
}

fn serial() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
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

fn create_domain(name: &str) -> String {
    let out = run_args(&["create", "Domain", "--field", &format!("name={name}")]);
    assert_ok(&out, "create Domain");
    stdout_string(&out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim_end_matches(['\n', '\r'])
        .to_string()
}

fn get_json(object: &str, id: Option<&str>) -> Value {
    let mut args = vec!["get", object];
    if let Some(i) = id {
        args.push(i);
    }
    args.push("--json");
    let out = run_args(&args);
    assert_ok(&out, "get --json");
    serde_json::from_str(stdout_string(&out).trim()).expect("get --json emits a single JSON line")
}

fn upsert_domain_plan(name: &str, description: &str) -> Vec<u8> {
    format!(
        "{{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
         \"value\":{{\"d1\":{{\"name\":\"{name}\",\"description\":\"{description}\"}}}}}}\n"
    )
    .into_bytes()
}

fn apply_done(plan: &[u8]) -> (u64, u64, u64) {
    let out = run_with_stdin(&["apply", "--stdin", "--json"], plan);
    assert_ok(&out, "apply --json");
    for line in stdout_string(&out).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("apply --json line");
        if v.get("op").and_then(Value::as_str) == Some("summary") {
            let done = &v["done"];
            return (
                done["created"].as_u64().unwrap_or(0),
                done["updated"].as_u64().unwrap_or(0),
                done["failed"].as_u64().unwrap_or(0),
            );
        }
    }
    panic!("apply --json produced no summary record");
}

fn apply_summary(plan: &[u8]) -> (u64, u64, u64, u64) {
    let out = run_with_stdin(&["apply", "--stdin", "--json"], plan);
    assert_ok(&out, "apply --json");
    for line in stdout_string(&out).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("apply --json line");
        if v.get("op").and_then(Value::as_str) == Some("summary") {
            let done = &v["done"];
            return (
                done["created"].as_u64().unwrap_or(0),
                done["updated"].as_u64().unwrap_or(0),
                done["destroyed"].as_u64().unwrap_or(0),
                done["failed"].as_u64().unwrap_or(0),
            );
        }
    }
    panic!("apply --json produced no summary record");
}

fn parse_missing_ref(err: &str) -> Option<String> {
    let start = err.find("references ")? + "references ".len();
    let rest = &err[start..];
    let end = rest.find(" but ")?;
    Some(rest[..end].trim().to_string())
}

fn snapshot_output(object: &str) -> Output {
    let mut allow = String::new();
    for _ in 0..25 {
        let allow_owned = allow.clone();
        let mut args: Vec<&str> = vec!["snapshot", object];
        if !allow_owned.is_empty() {
            args.push("--allow-unresolved");
            args.push(allow_owned.as_str());
        }
        let out = run_args(&args);
        if out.status.success() {
            return out;
        }
        let err = stderr_string(&out);
        match parse_missing_ref(&err) {
            Some(missing) if !allow.split(',').any(|a| missing == a) => {
                if !allow.is_empty() {
                    allow.push(',');
                }
                allow.push_str(&missing);
            }
            _ => panic!("snapshot {object} failed: {err}"),
        }
    }
    panic!("snapshot {object}: unresolved references did not converge");
}

fn id_with(object: &str, field: &str, value: &str) -> Option<String> {
    ids_with(object, field, value).into_iter().next()
}

fn ids_with(object: &str, field: &str, value: &str) -> Vec<String> {
    let out = run_args(&[
        "query",
        object,
        "--fields",
        &format!("id,{field}"),
        "--json",
    ]);
    if !out.status.success() {
        return Vec::new();
    }
    stdout_string(&out)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get(field).and_then(Value::as_str) == Some(value))
        .filter_map(|v| v.get("id").and_then(Value::as_str).map(String::from))
        .collect()
}

fn create_account_user(name: &str, domain_id: &str) -> String {
    let out = run_args(&[
        "create",
        "Account/User",
        "--field",
        &format!("name={name}"),
        "--field",
        &format!("domainId={domain_id}"),
    ]);
    assert_ok(&out, "create Account/User");
    stdout_string(&out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim_end_matches(['\n', '\r'])
        .to_string()
}

fn purge_domain(domain_id: &str) {
    for acc in ids_with("Account", "domainId", domain_id) {
        let _ = run_args(&["delete", "Account", "--ids", &acc]);
    }
    let dkim = ids_with("DkimSignature", "domainId", domain_id);
    if !dkim.is_empty() {
        let _ = run_args(&["delete", "DkimSignature", "--ids", &dkim.join(",")]);
    }
    let _ = run_args(&["delete", "Domain", "--ids", domain_id]);
}

struct DomainTree(String);

impl Drop for DomainTree {
    fn drop(&mut self) {
        purge_domain(&self.0);
    }
}

fn poll_dkim_selector(domain_id: &str) -> Option<String> {
    for _ in 0..40 {
        let out = run_args(&[
            "query",
            "DkimSignature",
            "--fields",
            "selector,@type,domainId",
            "--json",
        ]);
        if out.status.success() {
            let hit = stdout_string(&out)
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find(|v| {
                    v.get("domainId").and_then(Value::as_str) == Some(domain_id)
                        && v.get("@type").and_then(Value::as_str) == Some("Dkim1Ed25519Sha256")
                })
                .and_then(|v| v.get("selector").and_then(Value::as_str).map(String::from));
            if hit.is_some() {
                return hit;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    None
}

struct ObjectGuard {
    object: &'static str,
    id: String,
}

impl Drop for ObjectGuard {
    fn drop(&mut self) {
        let _ = run_args(&["delete", self.object, "--ids", &self.id]);
    }
}

struct DomainGuard(String);

impl Drop for DomainGuard {
    fn drop(&mut self) {
        let dkim = ids_where("DkimSignature", &format!("domainId={}", self.0));
        if !dkim.is_empty() {
            let _ = run_args(&["delete", "DkimSignature", "--ids", &dkim.join(",")]);
        }
        let _ = run_args(&["delete", "Domain", "--ids", &self.0]);
    }
}

#[test]
fn snapshot_domain_is_upsert_only_with_match_on() {
    require_server!();
    let _serial = serial();
    let name = format!("snap-{}.example.com", unique_suffix());
    let id = create_domain(&name);
    let _guard = DomainGuard(id);

    let out = snapshot_output("Domain");
    assert_ok(&out, "snapshot Domain");
    let plan = stdout_string(&out);

    let mut domain_match_on: Option<Value> = None;
    let mut included_my_domain = false;
    for line in plan.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("snapshot line is NDJSON");
        assert_ne!(
            v["@type"].as_str(),
            Some("destroy"),
            "snapshot must never emit destroy ops: {line}"
        );
        if v["@type"].as_str() == Some("upsert") && v["object"].as_str() == Some("Domain") {
            domain_match_on = v.get("matchOn").cloned();
            if let Some(value) = v.get("value").and_then(Value::as_object) {
                included_my_domain |= value
                    .values()
                    .any(|e| e.get("name").and_then(Value::as_str) == Some(name.as_str()));
            }
        }
    }
    assert_eq!(
        domain_match_on,
        Some(json!(["name"])),
        "Domain upsert op must declare matchOn from the label property"
    );
    assert!(
        included_my_domain,
        "snapshot must include the created domain keyed by name"
    );

    let dry = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan.as_bytes());
    assert_ok(
        &dry,
        "the emitted snapshot must be consumable by apply --dry-run",
    );
}

#[test]
fn upsert_creates_then_updates_idempotently() {
    require_server!();
    let _serial = serial();
    let name = format!("ups-{}.example.com", unique_suffix());

    let (created, updated, failed) = apply_done(&upsert_domain_plan(&name, "v1"));
    assert_eq!(
        (created, updated, failed),
        (1, 0, 0),
        "first upsert of a new name must create exactly one object"
    );

    let id = ids_where("Domain", &format!("name={name}"))
        .into_iter()
        .next()
        .expect("created domain must be queryable by name");
    let _guard = DomainGuard(id.clone());

    let (created2, updated2, failed2) = apply_done(&upsert_domain_plan(&name, "v2"));
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "second upsert must match and not create"
    );
    assert!(
        updated2 >= 1,
        "second upsert must update the matched object"
    );
    assert_eq!(
        ids_where("Domain", &format!("name={name}")).len(),
        1,
        "upsert must not create a duplicate domain"
    );
    assert_eq!(
        get_json("Domain", Some(&id))["description"].as_str(),
        Some("v2"),
        "matched object must be patched with the new value"
    );
}

#[test]
fn upsert_updates_domain_referenced_by_non_nullable_singleton() {
    require_server!();
    let _serial = serial();
    let name = format!("ref-{}.example.com", unique_suffix());
    let id = create_domain(&name);

    let set_ref = run_args(&[
        "update",
        "SystemSettings",
        "--field",
        &format!("defaultDomainId={id}"),
        "--field",
        &format!("defaultHostname=mail.{name}"),
    ]);
    assert_ok(
        &set_ref,
        "point SystemSettings.defaultDomainId at the domain",
    );

    let destroy = run_args(&["delete", "Domain", "--ids", &id]);
    assert!(
        !destroy.status.success(),
        "a domain referenced by the singleton must not be destroyable (objectIsLinked); \
         this is the destroy+create trap upsert avoids"
    );

    let (created, updated, failed) = apply_done(&upsert_domain_plan(&name, "reconciled"));
    assert_eq!(
        (created, failed),
        (0, 0),
        "upsert of the referenced domain must update in place, not create or fail"
    );
    assert!(updated >= 1, "upsert must update the referenced domain");

    assert_eq!(
        get_json("SystemSettings", None)["defaultDomainId"].as_str(),
        Some(id.as_str()),
        "the non-nullable singleton reference must survive the upsert"
    );
    assert_eq!(
        get_json("Domain", Some(&id))["description"].as_str(),
        Some("reconciled"),
        "the referenced domain must be reconciled to the planned value"
    );

    let (created2, _, failed2) = apply_done(&upsert_domain_plan(&name, "reconciled"));
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-applying the plan must be idempotent"
    );
}

#[test]
fn upsert_multi_variant_mta_route_matches_per_variant_and_is_idempotent() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let local_name = format!("local-{suffix}.test");
    let mx_name = format!("mx-{suffix}.test");

    let create_plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"r1\":{{\"@type\":\"Local\",\"name\":\"{local_name}\",\"description\":\"v1\"}},\
         \"r2\":{{\"@type\":\"Mx\",\"name\":\"{mx_name}\",\"description\":\"v1\",\
         \"ipLookupStrategy\":\"v4ThenV6\",\"maxMultihomed\":2,\"maxMxHosts\":5}}}}}}\n"
    )
    .into_bytes();

    let (created, _updated, failed) = apply_done(&create_plan);
    assert_eq!(
        (created, failed),
        (2, 0),
        "first upsert must create both variant entries"
    );

    let local_id = id_with("MtaRoute", "name", &local_name).expect("Local route created");
    let mx_id = id_with("MtaRoute", "name", &mx_name).expect("Mx route created");
    let _g1 = ObjectGuard {
        object: "MtaRoute",
        id: local_id,
    };
    let _g2 = ObjectGuard {
        object: "MtaRoute",
        id: mx_id.clone(),
    };

    let update_plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"r1\":{{\"@type\":\"Local\",\"name\":\"{local_name}\",\"description\":\"v2\"}},\
         \"r2\":{{\"@type\":\"Mx\",\"name\":\"{mx_name}\",\"description\":\"v2\",\
         \"ipLookupStrategy\":\"v6ThenV4\",\"maxMultihomed\":3,\"maxMxHosts\":7}}}}}}\n"
    )
    .into_bytes();

    let (created2, updated2, failed2) = apply_done(&update_plan);
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-applying with changed mutable fields must match per variant, not create \
         (immutable `name` is the key and must not break the update)"
    );
    assert!(updated2 >= 2, "both variant entries must update");

    assert!(
        id_with("MtaRoute", "name", &local_name).is_some()
            && id_with("MtaRoute", "name", &mx_name).is_some(),
        "both routes must still resolve by name after re-apply"
    );
    assert_eq!(
        get_json("MtaRoute", Some(&mx_id))["maxMxHosts"].as_u64(),
        Some(7),
        "Mx route mutable field must be reconciled"
    );
}

#[test]
fn snapshot_label_property_other_than_name_uses_that_key() {
    require_server!();
    let _serial = serial();
    if ids_for("SpamTag").is_empty() {
        eprintln!("skipping: no SpamTag objects present");
        return;
    }

    let out = snapshot_output("SpamTag");
    assert_ok(&out, "snapshot SpamTag");
    let plan = stdout_string(&out);

    let mut saw_spam_tag = false;
    for line in plan.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("snapshot line is NDJSON");
        assert_ne!(v["@type"].as_str(), Some("destroy"), "no destroys: {line}");
        if v["object"].as_str() == Some("SpamTag") {
            saw_spam_tag = true;
            assert_eq!(
                v.get("matchOn"),
                Some(&json!(["tag"])),
                "SpamTag must match on its label property `tag`, not `name`: {line}"
            );
        }
    }
    assert!(saw_spam_tag, "expected a SpamTag upsert op");

    let (created, _, failed) = apply_done(plan.as_bytes());
    assert_eq!(
        (created, failed),
        (0, 0),
        "re-applying a SpamTag snapshot must match on tag and not create"
    );
}

#[test]
fn upsert_on_singleton_is_rejected() {
    require_server!();
    let plan =
        b"{\"@type\":\"upsert\",\"object\":\"SystemSettings\",\"value\":{\"s1\":{\"defaultHostname\":\"x\"}}}\n";
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan);
    assert!(
        !out.status.success(),
        "upsert on a singleton must be rejected"
    );
    assert!(
        stderr_string(&out).contains("cannot upsert singleton"),
        "expected a singleton-rejection message: {}",
        stderr_string(&out)
    );
}

#[test]
fn snapshot_value_match_warns_and_round_trips() {
    require_server!();
    let _serial = serial();
    if ids_for("Tracer").is_empty() {
        eprintln!("skipping: no default Tracer to snapshot");
        return;
    }

    let out = snapshot_output("Tracer");
    assert_ok(&out, "snapshot Tracer");
    let warning = stderr_string(&out);
    assert!(
        warning.contains("no label property") || warning.contains("matched by value"),
        "snapshot of a keyless type must warn about value matching: {warning}"
    );

    let plan = stdout_string(&out);
    for line in plan.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("snapshot line is NDJSON");
        if v["object"].as_str() == Some("Tracer") {
            assert!(
                v.get("matchOn").is_none(),
                "a keyless type must omit matchOn (value matching): {line}"
            );
        }
    }

    let (created, _, failed) = apply_done(plan.as_bytes());
    assert_eq!(
        (created, failed),
        (0, 0),
        "value-matching a freshly captured snapshot must update in place, not create"
    );
    let (created2, _, failed2) = apply_done(plan.as_bytes());
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-applying a value-matched snapshot must be idempotent"
    );
}

#[test]
fn upsert_resolves_client_ref_to_a_matched_not_created_parent() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let domain_name = format!("xref-{suffix}.test");
    let user_name = format!("u-{suffix}");

    let plan = |desc: &str| -> Vec<u8> {
        format!(
            "{{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
             \"value\":{{\"dom\":{{\"name\":\"{domain_name}\"}}}}}}\n\
             {{\"@type\":\"upsert\",\"object\":\"Account\",\"matchOn\":[\"name\"],\
             \"value\":{{\"acc\":{{\"@type\":\"User\",\"name\":\"{user_name}\",\
             \"domainId\":\"#dom\",\"description\":\"{desc}\"}}}}}}\n"
        )
        .into_bytes()
    };

    let (created, _updated, failed) = apply_done(&plan("v1"));
    assert_eq!(
        (created, failed),
        (2, 0),
        "first apply must create the domain and the account that references it"
    );

    let domain_id = id_with("Domain", "name", &domain_name).expect("domain created");
    let _tree = DomainTree(domain_id.clone());

    let (created2, updated2, failed2) = apply_done(&plan("v2"));
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-apply must match the parent (not recreate) and resolve #dom to the matched id"
    );
    assert!(updated2 >= 1, "the account must update");

    let account_id = id_with("Account", "name", &user_name).expect("account exists");
    let account = get_json("Account", Some(&account_id));
    assert_eq!(
        account["domainId"].as_str(),
        Some(domain_id.as_str()),
        "the account must still point at the matched parent domain"
    );
    assert_eq!(account["description"].as_str(), Some("v2"));
    assert_eq!(
        ids_with("Account", "name", &user_name).len(),
        1,
        "the account must not be duplicated"
    );
}

#[test]
fn upsert_non_unique_label_is_ambiguous_and_compound_match_disambiguates() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let d1 = create_domain(&format!("dk1-{suffix}.test"));
    let _t1 = DomainTree(d1.clone());
    let d2 = create_domain(&format!("dk2-{suffix}.test"));
    let _t2 = DomainTree(d2.clone());

    let (s1, s2) = match (poll_dkim_selector(&d1), poll_dkim_selector(&d2)) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("skipping: auto DKIM signatures did not appear for both domains");
            return;
        }
    };
    if s1 != s2 {
        eprintln!("skipping: domains did not share a DKIM selector ({s1} vs {s2})");
        return;
    }

    let ambiguous = format!(
        "{{\"@type\":\"upsert\",\"object\":\"DkimSignature\",\
         \"value\":{{\"x\":{{\"@type\":\"Dkim1Ed25519Sha256\",\"selector\":\"{s1}\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin"], ambiguous.as_bytes());
    assert!(
        !out.status.success(),
        "matching a non-unique label without disambiguation must fail"
    );
    assert!(
        stderr_string(&out).contains("ambiguous"),
        "expected an ambiguous-match error: {}",
        stderr_string(&out)
    );

    let compound = format!(
        "{{\"@type\":\"upsert\",\"object\":\"DkimSignature\",\"matchOn\":[\"selector\",\"domainId\"],\
         \"value\":{{\
         \"a\":{{\"@type\":\"Dkim1Ed25519Sha256\",\"selector\":\"{s1}\",\"domainId\":\"{d1}\",\"report\":true}},\
         \"b\":{{\"@type\":\"Dkim1Ed25519Sha256\",\"selector\":\"{s1}\",\"domainId\":\"{d2}\",\"report\":true}}}}}}\n"
    );
    let (created, _updated, failed) = apply_done(compound.as_bytes());
    assert_eq!(
        (created, failed),
        (0, 0),
        "a compound key (selector + domainId) must match each signature uniquely"
    );
    let (created2, _u2, failed2) = apply_done(compound.as_bytes());
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "compound-keyed upsert must be idempotent"
    );
}

#[test]
fn upsert_multi_variant_entry_without_at_type_is_rejected() {
    require_server!();
    let _serial = serial();
    let name = format!("noat-{}.test", unique_suffix());
    let plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\
         \"value\":{{\"r\":{{\"name\":\"{name}\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin"], plan.as_bytes());
    assert!(
        !out.status.success(),
        "a multi-variant upsert entry without @type must be rejected"
    );
    let err = stderr_string(&out);
    assert!(
        err.contains("missing") && err.contains("@type"),
        "expected a missing-@type error: {err}"
    );
    assert!(
        id_with("MtaRoute", "name", &name).is_none(),
        "the rejected entry must not have created anything"
    );
}

#[test]
fn upsert_strips_server_set_fields_from_a_matched_update() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let domain = create_domain(&format!("ss-{suffix}.test"));
    let _tree = DomainTree(domain.clone());
    let user_name = format!("ssu-{suffix}");
    let account_id = create_account_user(&user_name, &domain);

    let real_email = get_json("Account", Some(&account_id))["emailAddress"]
        .as_str()
        .expect("server-assigned emailAddress")
        .to_string();

    let plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"Account\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"User\",\"name\":\"{user_name}\",\"description\":\"reconciled\",\
         \"emailAddress\":\"stale@bogus.invalid\",\"createdAt\":\"2000-01-01T00:00:00Z\"}}}}}}\n"
    );
    let (created, updated, failed) = apply_done(plan.as_bytes());
    assert_eq!(
        (created, failed),
        (0, 0),
        "the body carries stale serverSet fields; they must be dropped so the update succeeds"
    );
    assert!(updated >= 1, "the account must update");

    let account = get_json("Account", Some(&account_id));
    assert_eq!(
        account["description"].as_str(),
        Some("reconciled"),
        "the mutable field must be reconciled"
    );
    assert_eq!(
        account["emailAddress"].as_str(),
        Some(real_email.as_str()),
        "the serverSet emailAddress must be untouched by the upsert"
    );
}

#[test]
fn apply_combined_domain_upsert_then_singleton_reference_is_idempotent() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let domain_name = format!("decl-{suffix}.test");

    let plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
         \"value\":{{\"dom\":{{\"name\":\"{domain_name}\"}}}}}}\n\
         {{\"@type\":\"update\",\"object\":\"SystemSettings\",\"id\":\"singleton\",\
         \"value\":{{\"defaultDomainId\":\"#dom\",\"defaultHostname\":\"mail.{domain_name}\"}}}}\n"
    );

    let (created, _updated, failed) = apply_done(plan.as_bytes());
    assert_eq!(
        (created, failed),
        (1, 0),
        "first apply must create the domain and wire the singleton to it"
    );

    let domain_id = id_with("Domain", "name", &domain_name).expect("domain created");
    assert_eq!(
        get_json("SystemSettings", None)["defaultDomainId"].as_str(),
        Some(domain_id.as_str()),
        "the singleton must reference the upserted domain via the resolved client id"
    );

    let (created2, _u2, failed2) = apply_done(plan.as_bytes());
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-applying the same declarative plan must be a no-op create-wise (idempotent), \
         even though the domain is now pinned by the non-nullable singleton reference"
    );
    assert_eq!(
        get_json("SystemSettings", None)["defaultDomainId"].as_str(),
        Some(domain_id.as_str()),
        "the singleton reference must be preserved across re-apply"
    );
    assert_eq!(
        ids_with("Domain", "name", &domain_name).len(),
        1,
        "the domain must not be duplicated by re-apply"
    );
}

struct NameGuard {
    object: &'static str,
    field: &'static str,
    value: String,
}

impl Drop for NameGuard {
    fn drop(&mut self) {
        for id in ids_with(self.object, self.field, &self.value) {
            let _ = run_args(&["delete", self.object, "--ids", &id]);
        }
    }
}

#[test]
fn snapshot_delete_apply_restores_deleted_objects() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let n1 = format!("restore1-{suffix}.test");
    let n2 = format!("restore2-{suffix}.test");
    let _g1 = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: n1.clone(),
    };
    let _g2 = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: n2.clone(),
    };

    let seed = format!(
        "{{\"@type\":\"upsert\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{n1}\",\"description\":\"orig\"}},\
         \"b\":{{\"@type\":\"Local\",\"name\":\"{n2}\",\"description\":\"orig\"}}}}}}\n"
    );
    let (created, _u, failed) = apply_done(seed.as_bytes());
    assert_eq!((created, failed), (2, 0), "seed must create two routes");

    let plan = stdout_string(&snapshot_output("MtaRoute"));

    let id1 = id_with("MtaRoute", "name", &n1).expect("route 1");
    let id2 = id_with("MtaRoute", "name", &n2).expect("route 2");
    let del = run_args(&["delete", "MtaRoute", "--ids", &format!("{id1},{id2}")]);
    assert_ok(&del, "delete the two routes");
    assert!(
        id_with("MtaRoute", "name", &n1).is_none() && id_with("MtaRoute", "name", &n2).is_none(),
        "both routes must be gone before restore"
    );

    let (rcreated, _ru, rfailed) = apply_done(plan.as_bytes());
    assert_eq!(rfailed, 0, "restore must not fail");
    assert!(
        rcreated >= 2,
        "the two deleted routes must be recreated by re-applying the snapshot"
    );
    assert!(
        id_with("MtaRoute", "name", &n1).is_some() && id_with("MtaRoute", "name", &n2).is_some(),
        "both routes must be restored by name"
    );
}

#[test]
fn apply_destroy_op_removes_every_instance() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let seed = format!(
        "{{\"@type\":\"upsert\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"destroy1-{suffix}.test\"}},\
         \"b\":{{\"@type\":\"Local\",\"name\":\"destroy2-{suffix}.test\"}}}}}}\n"
    );
    let (created, _u, failed) = apply_done(seed.as_bytes());
    assert_eq!((created, failed), (2, 0), "seed must create two routes");
    assert!(
        !ids_for("MtaRoute").is_empty(),
        "routes must exist before destroy"
    );

    let out = run_with_stdin(
        &["apply", "--stdin"],
        b"{\"@type\":\"destroy\",\"object\":\"MtaRoute\"}\n",
    );
    assert_ok(&out, "apply a real destroy op");
    assert!(
        ids_for("MtaRoute").is_empty(),
        "a destroy op must remove every instance of the type"
    );
}

#[test]
fn apply_continue_on_error_attempts_all_and_counts_failures() {
    require_server!();
    let plan = b"{\"@type\":\"update\",\"object\":\"Domain\",\"id\":\"missing-id-1\",\"value\":{\"description\":\"x\"}}\n\
                 {\"@type\":\"update\",\"object\":\"Domain\",\"id\":\"missing-id-2\",\"value\":{\"description\":\"y\"}}\n";
    let out = run_with_stdin(&["apply", "--stdin", "--continue-on-error", "--json"], plan);
    assert!(
        !out.status.success(),
        "apply must exit non-zero when operations fail"
    );
    let mut failed = None;
    for line in stdout_string(&out).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).expect("apply --json line");
        if v.get("op").and_then(Value::as_str) == Some("summary") {
            failed = v["done"]["failed"].as_u64();
        }
    }
    assert_eq!(
        failed,
        Some(2),
        "--continue-on-error must attempt both ops and count both failures"
    );
}

#[test]
fn upsert_match_on_reference_id_resolves_client_ref() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let domain_name = format!("keyref-{suffix}.test");
    let user_name = format!("ku-{suffix}");

    let plan = |desc: &str| -> Vec<u8> {
        format!(
            "{{\"@type\":\"upsert\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\
             \"value\":{{\"dom\":{{\"name\":\"{domain_name}\"}}}}}}\n\
             {{\"@type\":\"upsert\",\"object\":\"Account\",\"matchOn\":[\"name\",\"domainId\"],\
             \"value\":{{\"acc\":{{\"@type\":\"User\",\"name\":\"{user_name}\",\
             \"domainId\":\"#dom\",\"description\":\"{desc}\"}}}}}}\n"
        )
        .into_bytes()
    };

    let (created, _u, failed) = apply_done(&plan("v1"));
    assert_eq!(
        (created, failed),
        (2, 0),
        "first apply must create the domain and the account keyed on it"
    );

    let domain_id = id_with("Domain", "name", &domain_name).expect("domain created");
    let _tree = DomainTree(domain_id.clone());

    let (created2, updated2, failed2) = apply_done(&plan("v2"));
    assert_eq!(
        (created2, failed2),
        (0, 0),
        "re-apply must resolve `#dom` inside the compound matchOn and update in place; \
         before the fix the unresolved `#dom` broke the match and forced a duplicate create"
    );
    assert!(updated2 >= 1, "the account must update");
    assert_eq!(
        ids_with("Account", "name", &user_name).len(),
        1,
        "a broken reference match must not duplicate the account"
    );
    let account_id = id_with("Account", "name", &user_name).expect("account exists");
    let account = get_json("Account", Some(&account_id));
    assert_eq!(
        account["domainId"].as_str(),
        Some(domain_id.as_str()),
        "the matched account must still point at the resolved domain"
    );
    assert_eq!(account["description"].as_str(), Some("v2"));
}

#[test]
fn upsert_match_on_unresolved_reference_is_rejected() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let user_name = format!("noref-{suffix}");
    let plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"Account\",\"matchOn\":[\"name\",\"domainId\"],\
         \"value\":{{\"acc\":{{\"@type\":\"User\",\"name\":\"{user_name}\",\
         \"domainId\":\"#ghost\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin"], plan.as_bytes());
    assert!(
        !out.status.success(),
        "a matchOn reference with no producing op must be rejected, not silently mismatched"
    );
    let err = stderr_string(&out);
    assert!(
        err.contains("unresolved id `#ghost`"),
        "expected an unresolved-reference error: {err}"
    );
}

#[test]
fn dry_run_rejects_unresolved_match_reference() {
    require_server!();
    let user_name = format!("dryref-{}", unique_suffix());
    let plan = format!(
        "{{\"@type\":\"upsert\",\"object\":\"Account\",\"matchOn\":[\"name\",\"domainId\"],\
         \"value\":{{\"acc\":{{\"@type\":\"User\",\"name\":\"{user_name}\",\
         \"domainId\":\"#ghost\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan.as_bytes());
    assert!(
        !out.status.success(),
        "dry-run must catch an unresolved matchOn reference before reporting success"
    );
    assert!(
        stderr_string(&out).contains("unresolved id `#ghost`"),
        "expected the unresolved-reference error at dry-run time: {}",
        stderr_string(&out)
    );
}

#[test]
fn reconcile_converges_and_scopes_deletes_per_variant() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let a = format!("rc-a-{suffix}.test");
    let b = format!("rc-b-{suffix}.test");
    let mx = format!("rc-mx-{suffix}.test");
    let _ga = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: a.clone(),
    };
    let _gb = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: b.clone(),
    };
    let _gmx = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: mx.clone(),
    };

    let seed = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}},\
         \"b\":{{\"@type\":\"Local\",\"name\":\"{b}\"}}}}}}\n"
    )
    .into_bytes();
    let (c, _u, d, f) = apply_summary(&seed);
    assert_eq!(
        (c, d, f),
        (2, 0, 0),
        "the seed reconcile must create two Local routes and destroy nothing"
    );

    let mx_created = run_args(&[
        "create",
        "MtaRoute/Mx",
        "--field",
        &format!("name={mx}"),
        "--field",
        "ipLookupStrategy=v4ThenV6",
        "--field",
        "maxMultihomed=2",
        "--field",
        "maxMxHosts=5",
    ]);
    assert_ok(&mx_created, "create an out-of-band Mx route");

    let converge = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}}}}}}\n"
    )
    .into_bytes();
    let (_c2, _u2, d2, f2) = apply_summary(&converge);
    assert_eq!(f2, 0, "the converging reconcile must not fail");
    assert!(
        d2 >= 1,
        "the Local route dropped from the plan must be destroyed, not leaked"
    );
    assert!(
        id_with("MtaRoute", "name", &a).is_some(),
        "the retained Local route must remain"
    );
    assert!(
        id_with("MtaRoute", "name", &b).is_none(),
        "the dropped Local route must be gone"
    );
    assert!(
        id_with("MtaRoute", "name", &mx).is_some(),
        "the Mx variant is absent from the plan's @types and must be left untouched"
    );

    let (c3, _u3, d3, f3) = apply_summary(&converge);
    assert_eq!(
        (c3, d3, f3),
        (0, 0, 0),
        "re-applying the converged reconcile must be a no-op"
    );
}

#[test]
fn reconcile_rename_does_not_leak_old_object() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let old = format!("rcren-old-{suffix}.test");
    let new = format!("rcren-new-{suffix}.test");
    let _go = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: old.clone(),
    };
    let _gn = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: new.clone(),
    };

    let plan = |name: &str| -> Vec<u8> {
        format!(
            "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
             \"r\":{{\"@type\":\"Local\",\"name\":\"{name}\"}}}}}}\n"
        )
        .into_bytes()
    };

    let (c1, _u1, _d1, f1) = apply_summary(&plan(&old));
    assert_eq!((c1, f1), (1, 0), "first reconcile must create the route");
    assert!(id_with("MtaRoute", "name", &old).is_some());

    let (_c2, _u2, d2, f2) = apply_summary(&plan(&new));
    assert_eq!(f2, 0, "the rename reconcile must not fail");
    assert!(
        d2 >= 1,
        "renaming (dropping the old name from the source) must destroy the old object; \
         this is exactly the leak that plain upsert cannot fix"
    );
    assert!(
        id_with("MtaRoute", "name", &new).is_some(),
        "the renamed route must exist"
    );
    assert!(
        id_with("MtaRoute", "name", &old).is_none(),
        "the old route must not leak"
    );
}

#[test]
fn reconcile_dry_run_makes_no_changes() {
    require_server!();
    let _serial = serial();
    let name = format!("rc-dry-{}.test", unique_suffix());
    let plan = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"r\":{{\"@type\":\"Local\",\"name\":\"{name}\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan.as_bytes());
    assert_ok(&out, "reconcile dry-run must be accepted");
    assert!(
        id_with("MtaRoute", "name", &name).is_none(),
        "a dry-run reconcile must not create anything"
    );
}

#[test]
fn reconcile_on_singleton_is_rejected() {
    require_server!();
    let plan =
        b"{\"@type\":\"reconcile\",\"object\":\"SystemSettings\",\"value\":{\"s1\":{\"defaultHostname\":\"x\"}}}\n";
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan);
    assert!(
        !out.status.success(),
        "reconcile on a singleton must be rejected"
    );
    assert!(
        stderr_string(&out).contains("cannot reconcile singleton"),
        "expected a singleton-rejection message: {}",
        stderr_string(&out)
    );
}

#[test]
fn reconcile_creates_and_deletes_in_one_op() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let a = format!("rccd-a-{suffix}.test");
    let b = format!("rccd-b-{suffix}.test");
    let c = format!("rccd-c-{suffix}.test");
    let _ga = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: a.clone(),
    };
    let _gb = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: b.clone(),
    };
    let _gc = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: c.clone(),
    };

    let seed = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}},\
         \"b\":{{\"@type\":\"Local\",\"name\":\"{b}\"}}}}}}\n"
    )
    .into_bytes();
    let (c0, _u0, d0, f0) = apply_summary(&seed);
    assert_eq!((c0, d0, f0), (2, 0, 0), "seed creates two Local routes");

    let step = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}},\
         \"c\":{{\"@type\":\"Local\",\"name\":\"{c}\"}}}}}}\n"
    )
    .into_bytes();
    let (c1, _u1, d1, f1) = apply_summary(&step);
    assert_eq!(f1, 0, "the combined create+delete reconcile must not fail");
    assert_eq!(c1, 1, "exactly the new route (c) must be created");
    assert_eq!(d1, 1, "exactly the dropped route (b) must be destroyed");
    assert!(id_with("MtaRoute", "name", &a).is_some(), "a is retained");
    assert!(id_with("MtaRoute", "name", &c).is_some(), "c is created");
    assert!(id_with("MtaRoute", "name", &b).is_none(), "b is deleted");
}

#[test]
fn reconcile_multi_variant_empty_value_deletes_nothing() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let local = format!("rcev-l-{suffix}.test");
    let mx = format!("rcev-m-{suffix}.test");
    let _gl = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: local.clone(),
    };
    let _gm = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: mx.clone(),
    };

    let seed = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"l\":{{\"@type\":\"Local\",\"name\":\"{local}\"}}}}}}\n"
    )
    .into_bytes();
    assert_eq!(apply_summary(&seed).3, 0, "seed must not fail");
    let mx_created = run_args(&[
        "create",
        "MtaRoute/Mx",
        "--field",
        &format!("name={mx}"),
        "--field",
        "ipLookupStrategy=v4ThenV6",
        "--field",
        "maxMultihomed=2",
        "--field",
        "maxMxHosts=5",
    ]);
    assert_ok(&mx_created, "create Mx route");

    let empty =
        b"{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{}}\n";
    let (c, _u, d, f) = apply_summary(empty);
    assert_eq!(
        (c, d, f),
        (0, 0, 0),
        "an empty-value reconcile on a multi-variant type must be a pure no-op (no @types named)"
    );
    assert!(
        id_with("MtaRoute", "name", &local).is_some() && id_with("MtaRoute", "name", &mx).is_some(),
        "both variants must survive an empty multi-variant reconcile"
    );
}

#[test]
fn reconcile_cleanup_emits_json_record() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let a = format!("rcj-a-{suffix}.test");
    let b = format!("rcj-b-{suffix}.test");
    let _ga = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: a.clone(),
    };
    let _gb = NameGuard {
        object: "MtaRoute",
        field: "name",
        value: b.clone(),
    };

    let seed = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}},\
         \"b\":{{\"@type\":\"Local\",\"name\":\"{b}\"}}}}}}\n"
    )
    .into_bytes();
    assert_eq!(apply_summary(&seed).3, 0, "seed must not fail");

    let step = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"MtaRoute\",\"matchOn\":[\"name\"],\"value\":{{\
         \"a\":{{\"@type\":\"Local\",\"name\":\"{a}\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin", "--json"], step.as_bytes());
    assert_ok(&out, "converging reconcile");
    let cleanup = stdout_string(&out)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| {
            v.get("op").and_then(Value::as_str) == Some("reconcile")
                && v.get("stage").and_then(Value::as_str) == Some("cleanup")
        })
        .expect("a reconcile cleanup NDJSON record must be emitted");
    assert!(
        cleanup["destroyed"].as_u64().unwrap_or(0) >= 1,
        "the cleanup record must report the destroyed count: {cleanup}"
    );
}

#[test]
fn reconcile_without_match_key_is_rejected() {
    require_server!();
    let plan =
        b"{\"@type\":\"reconcile\",\"object\":\"Tracer\",\"value\":{\"t\":{\"description\":\"x\"}}}\n";
    let out = run_with_stdin(&["apply", "--stdin", "--dry-run"], plan);
    assert!(
        !out.status.success(),
        "reconcile on a keyless type without matchOn must be rejected (no silent value-match)"
    );
    let err = stderr_string(&out);
    assert!(
        err.contains("no match key"),
        "expected a value-fallback rejection: {err}"
    );
}

#[test]
fn reconcile_delete_blocked_by_external_reference_fails_cleanly() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let dn = format!("rcblock-d-{suffix}.test");
    let other = format!("rcblock-o-{suffix}.test");

    let d_id = create_domain(&dn);
    let _dtree = DomainTree(d_id.clone());
    let _acc = create_account_user(&format!("bu-{suffix}"), &d_id);

    let plan = format!(
        "{{\"@type\":\"reconcile\",\"object\":\"Domain\",\"matchOn\":[\"name\"],\"value\":{{\
         \"keep\":{{\"name\":\"{other}\"}}}}}}\n"
    );
    let out = run_with_stdin(&["apply", "--stdin", "--json"], plan.as_bytes());
    let _other_guard = id_with("Domain", "name", &other).map(DomainTree);

    assert!(
        !out.status.success(),
        "destroying a domain still referenced by an account must fail the reconcile"
    );
    let failed = stdout_string(&out)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("op").and_then(Value::as_str) == Some("summary"))
        .and_then(|v| v["done"]["failed"].as_u64())
        .unwrap_or(0);
    assert!(
        failed >= 1,
        "the blocked deletion must be counted as failed"
    );
    assert!(
        id_with("Domain", "name", &dn).is_some(),
        "the referenced domain must survive the failed deletion (no orphaning)"
    );
}

fn self_signed_pem() -> (String, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["itest-cert.example.com".to_string()])
        .expect("generate self-signed certificate");
    (ck.cert.pem(), ck.signing_key.serialize_pem())
}

struct PurgeAll(&'static str);

impl Drop for PurgeAll {
    fn drop(&mut self) {
        for id in ids_for(self.0) {
            let _ = run_args(&["delete", self.0, "--ids", &id]);
        }
    }
}

#[test]
fn upsert_certificate_matches_on_server_set_label() {
    require_server!();
    let _serial = serial();
    let _purge = PurgeAll("Certificate");

    let (cert_pem, key_pem) = self_signed_pem();
    let create_out = run_args(&[
        "create",
        "Certificate",
        "--json",
        &json!({
            "certificate": {"@type": "Text", "value": cert_pem},
            "privateKey": {"@type": "Text", "secret": key_pem},
        })
        .to_string(),
    ]);
    assert_ok(&create_out, "create Certificate");
    let cert_id = stdout_string(&create_out)
        .split_whitespace()
        .last()
        .expect("created id")
        .trim_end_matches(['\n', '\r'])
        .to_string();

    let sans = get_json("Certificate", Some(&cert_id))
        .get("subjectAlternativeNames")
        .cloned()
        .expect("the server derives subjectAlternativeNames");

    let plan = json!({
        "@type": "upsert",
        "object": "Certificate",
        "matchOn": ["subjectAlternativeNames"],
        "value": {
            "c1": {
                "certificate": {"@type": "Text", "value": cert_pem},
                "privateKey": {"@type": "Text", "secret": key_pem},
                "subjectAlternativeNames": sans,
            }
        }
    })
    .to_string()
        + "\n";

    let (c1, u1, f1) = apply_done(plan.as_bytes());
    assert_eq!(
        (c1, f1),
        (0, 0),
        "the plan must match the existing cert on its server-set SANs, not fail or duplicate"
    );
    assert!(u1 >= 1, "the matched certificate must update");
    let (c2, _u2, f2) = apply_done(plan.as_bytes());
    assert_eq!((c2, f2), (0, 0), "the certificate plan must be idempotent");
    assert_eq!(
        ids_for("Certificate").len(),
        1,
        "no duplicate certificate must be created"
    );

    let count_before = ids_for("Certificate").len();
    assert_ok(
        &run_args(&["delete", "Certificate", "--ids", &cert_id]),
        "delete the certificate",
    );
    let (c3, _u3, f3) = apply_done(plan.as_bytes());
    assert_eq!(
        f3, 0,
        "create path must strip the server-set SAN and succeed"
    );
    assert!(c3 >= 1, "the missing certificate must be created");
    assert_eq!(
        ids_for("Certificate").len(),
        count_before,
        "the certificate count must return to its prior value"
    );
}

#[test]
fn snapshot_warns_on_anonymized_secret() {
    require_server!();
    let _serial = serial();
    let suffix = unique_suffix();
    let domain = create_domain(&format!("dkimsec-{suffix}.test"));
    let _tree = DomainTree(domain.clone());
    if poll_dkim_selector(&domain).is_none() {
        eprintln!("skipping: no auto DKIM signature appeared");
        return;
    }

    let out = snapshot_output("DkimSignature");
    assert_ok(&out, "snapshot DkimSignature");
    let err = stderr_string(&out);
    assert!(
        err.contains("anonymized") && err.contains("****"),
        "snapshotting an object with a server-anonymized secret must warn: {err}"
    );
}

#[test]
fn snapshot_certificate_env_var_variant_round_trips() {
    require_server!();
    let _serial = serial();
    let _purge = PurgeAll("Certificate");

    let payload = json!({
        "certificate": {"@type": "EnvironmentVariable", "variableName": stalwart::CERT_ENV_VAR},
        "privateKey": {"@type": "EnvironmentVariable", "variableName": stalwart::KEY_ENV_VAR},
    })
    .to_string();
    assert_ok(
        &run_args(&["create", "Certificate", "--json", &payload]),
        "create env-var Certificate",
    );

    let snap = snapshot_output("Certificate");
    assert_ok(&snap, "snapshot Certificate");
    assert!(
        !stderr_string(&snap).contains("anonymized"),
        "env-var key material is not a secret, so snapshot must not warn: {}",
        stderr_string(&snap)
    );

    let plan = stdout_string(&snap);
    let op = plan
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("snapshot line is NDJSON"))
        .find(|v| v["object"].as_str() == Some("Certificate"))
        .expect("a Certificate op");
    assert_eq!(op.get("matchOn"), Some(&json!(["subjectAlternativeNames"])));
    let entry = op["value"]
        .as_object()
        .and_then(|m| m.values().next())
        .expect("a value entry");
    assert_eq!(
        entry["subjectAlternativeNames"].get(stalwart::CERT_SAN),
        Some(&json!(true)),
        "the server-derived SAN must be captured in the body: {entry}"
    );
    assert_eq!(
        entry["privateKey"]["@type"].as_str(),
        Some("EnvironmentVariable"),
        "the non-secret key reference must be preserved"
    );

    let (c1, _u1, f1) = apply_done(plan.as_bytes());
    assert_eq!(
        (c1, f1),
        (0, 0),
        "re-applying must match the existing certificate"
    );
    let (c2, _u2, f2) = apply_done(plan.as_bytes());
    assert_eq!((c2, f2), (0, 0), "the snapshot must be idempotent");

    let count = ids_for("Certificate").len();
    for id in ids_for("Certificate") {
        assert_ok(
            &run_args(&["delete", "Certificate", "--ids", &id]),
            "delete certificate",
        );
    }
    let (c3, _u3, f3) = apply_done(plan.as_bytes());
    assert_eq!(
        f3, 0,
        "restore must not fail (SAN stripped on create, re-derived)"
    );
    assert!(
        c3 >= 1,
        "the deleted certificate must be recreated from the snapshot"
    );
    assert_eq!(
        ids_for("Certificate").len(),
        count,
        "restore must bring the certificate count back"
    );
}
