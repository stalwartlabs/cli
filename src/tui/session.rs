/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::jmap::Client;
use crate::jmap::account::{AccountInfo, Edition, Op};
use crate::jmap::credential::Credential;
use crate::schema::Schema;
use crate::schema::view::{Component, Target, ViewKind};
use crate::tui::job::metrics::MetricsCache;
use crate::tui::job::objects::PickerCache;
use crate::view::LabelCache;
use std::sync::Arc;

pub struct Connected {
    pub client: Arc<Client>,
    pub schema: Arc<Schema>,
    pub labels: LabelCache,
    pub server: Box<str>,
    pub url: Box<str>,
    pub account: Box<str>,
    pub info: AccountInfo,
    pub credential: Arc<Credential>,
    pub active_account: Option<Box<str>>,
    pub metrics: MetricsCache,
    pub pickers: PickerCache,
}

impl Connected {
    pub fn new(
        client: Client,
        schema: Schema,
        info: AccountInfo,
        credential: Arc<Credential>,
        url: &str,
        account: &str,
    ) -> Self {
        Connected {
            active_account: client.session().primary_account.clone(),
            info,
            credential,
            client: Arc::new(client),
            schema: Arc::new(schema),
            labels: LabelCache::default(),
            server: host(url).into(),
            url: url.into(),
            account: account.into(),
            metrics: MetricsCache::default(),
            pickers: PickerCache::default(),
        }
    }
}

const LIVE_DELIVERY: &str = "liveDeliveryTest";
const LIVE_METRICS: &str = "liveMetrics";
const LIVE_TRACING: &str = "liveTracing";
const METRIC_OBJECT: &str = "x:Metric";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Hidden,
    Locked,
    Open,
}

pub fn access(schema: &Schema, info: &AccountInfo, view: &str) -> Access {
    let perms = &info.permissions;
    let (visible, enterprise) = match schema.target(view) {
        Target::Component(Component::LiveDelivery) => (perms.has(LIVE_DELIVERY), false),
        Target::Component(Component::LiveTracing) => (perms.has(LIVE_TRACING), true),
        Target::Component(Component::Dashboard(_)) => match schema.get_object(METRIC_OBJECT) {
            Some(metric) => (
                perms.has(LIVE_METRICS)
                    && metric
                        .permission_prefix()
                        .is_some_and(|prefix| perms.can(prefix, Op::Get)),
                metric.is_enterprise(),
            ),
            None => (false, false),
        },
        Target::Object(def) => {
            let op = match def.kind() {
                ViewKind::CreateOnly => Op::Create,
                ViewKind::List | ViewKind::Singleton => Op::Get,
            };
            (
                def.object
                    .permission_prefix()
                    .is_some_and(|prefix| perms.can(prefix, op)),
                def.object.is_enterprise(),
            )
        }
        Target::Component(Component::Other(_)) | Target::Unknown => (false, false),
    };
    match (visible, enterprise && info.edition == Edition::Community) {
        (false, _) => Access::Hidden,
        (true, true) => Access::Locked,
        (true, false) => Access::Open,
    }
}

impl Connected {
    pub fn access(&self, view: &str) -> Access {
        if !self.offers(view) {
            return Access::Hidden;
        }
        access(&self.schema, &self.info, view)
    }

    fn offers(&self, view: &str) -> bool {
        let Target::Object(def) = self.schema.target(view) else {
            return true;
        };
        self.active_account
            .as_deref()
            .and_then(|id| self.client.session().account(id))
            .is_none_or(|account| account.supports(def.object.name()))
    }
}

pub fn edition_label(edition: Edition) -> &'static str {
    match edition {
        Edition::Oss => "Open Source",
        Edition::Community => "Community",
        Edition::Enterprise => "Enterprise",
    }
}

pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split_once('/').map_or(rest, |(host, _)| host)
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/session/tests.rs"]
mod tests;
