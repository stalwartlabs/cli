/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::net::TcpListener;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::Value;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

use super::error::{ContainerError, ContainerResult};

const IMAGE_NAME: &str = "stalwartlabs/stalwart";
const IMAGE_TAG: &str = "latest";

const HTTPS_PORT: u16 = 443;

const CONFIG_PATH: &str = "/etc/stalwart/config.json";
const CONFIG_JSON: &str = r#"{"@type":"RocksDb","blobSize":16834,"bufferSize":134217728,"path":"/var/lib/stalwart","poolWorkers":null}"#;

pub const ADMIN_USER: &str = "admin";
pub const ADMIN_PASSWORD: &str = "admin";

pub const CERT_ENV_VAR: &str = "ITEST_CERT_PEM";
pub const KEY_ENV_VAR: &str = "ITEST_KEY_PEM";
pub const CERT_SAN: &str = "itest-cert.example.com";

fn self_signed_pem() -> (String, String) {
    let ck = rcgen::generate_simple_self_signed(vec![CERT_SAN.to_string()])
        .expect("generate self-signed certificate");
    (ck.cert.pem(), ck.signing_key.serialize_pem())
}

#[allow(dead_code)]
pub struct Stalwart {
    _container: Container<GenericImage>,
    pub host: String,
    pub https_port: u16,
    pub base_url: String,
}

impl Stalwart {
    pub fn start() -> ContainerResult<Self> {
        let https_port = pick_free_port()?;
        let host = "127.0.0.1".to_owned();
        let base_url = format!("https://{host}:{https_port}");

        let image = GenericImage::new(IMAGE_NAME, IMAGE_TAG)
            .with_exposed_port(HTTPS_PORT.tcp())
            .with_wait_for(WaitFor::seconds(2));

        let (cert_pem, key_pem) = self_signed_pem();
        let request = image
            .with_env_var("STALWART_PUBLIC_URL", &base_url)
            .with_env_var("STALWART_RECOVERY_ADMIN", "admin:admin")
            .with_env_var(CERT_ENV_VAR, cert_pem)
            .with_env_var(KEY_ENV_VAR, key_pem)
            .with_copy_to(CONFIG_PATH, CONFIG_JSON.as_bytes().to_vec())
            .with_mapped_port(https_port, HTTPS_PORT.tcp())
            .with_startup_timeout(Duration::from_secs(180));

        let container = request.start()?;

        let me = Self {
            _container: container,
            host,
            https_port,
            base_url,
        };

        me.wait_ready(Duration::from_secs(120))?;
        Ok(me)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn fetch_jmap_session(&self) -> ContainerResult<Value> {
        let client = build_client()?;
        let url = format!("{}/.well-known/jmap", self.base_url);
        let resp = client
            .get(&url)
            .basic_auth(ADMIN_USER, Some(ADMIN_PASSWORD))
            .send()?;
        let status = resp.status();
        let text = resp.text()?;
        if !status.is_success() {
            return Err(ContainerError::Protocol(format!(
                "jmap session status {status}: {text}"
            )));
        }
        serde_json::from_str(&text)
            .map_err(|e| ContainerError::Protocol(format!("jmap session parse: {e}")))
    }

    fn wait_ready(&self, total: Duration) -> ContainerResult<()> {
        let deadline = Instant::now() + total;
        let mut last_err = String::from("no probe attempted");
        while Instant::now() < deadline {
            match self.fetch_jmap_session() {
                Ok(v) if v.get("apiUrl").is_some() => return Ok(()),
                Ok(v) => last_err = format!("session missing apiUrl: {v}"),
                Err(e) => last_err = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(ContainerError::Protocol(format!(
            "stalwart did not become ready in {total:?}: {last_err}"
        )))
    }
}

static SHARED: OnceLock<Option<Stalwart>> = OnceLock::new();

pub fn shared() -> Option<&'static Stalwart> {
    SHARED
        .get_or_init(|| match Stalwart::start() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("skipping: could not start stalwart test container: {e}");
                None
            }
        })
        .as_ref()
}

fn pick_free_port() -> ContainerResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn build_client() -> ContainerResult<Client> {
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .timeout(Duration::from_secs(10))
        .build()?;
    Ok(client)
}
