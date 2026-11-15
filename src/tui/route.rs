/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::Schema;
use crate::schema::view::{Component, Target, ViewKind};
use std::borrow::Cow;
use std::collections::VecDeque;

pub const MAX_HISTORY: usize = 50;
const DASHBOARD_LINK: &str = "CustomComponent/Dashboard";
const DASHBOARD_PREFIX: &str = "Dashboard/";
const DELIVERY_LINK: &str = "CustomComponent/LiveDelivery";
const TRACING_LINK: &str = "CustomComponent/LiveTracing";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectRef {
    Id(Box<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteTarget {
    Dashboard(Option<Box<str>>),
    LiveDelivery,
    LiveTracing,
    List(Box<str>),
    Query {
        view: Box<str>,
        field: Box<str>,
        value: Box<str>,
    },
    Object {
        view: Box<str>,
        id: ObjectRef,
    },
    New(Box<str>),
    Singleton(Box<str>),
    Action(Box<str>),
    Tools,
    Bootstrap,
    Log,
    Upsell(Box<str>),
    Unknown(Box<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub layout: Option<u8>,
    pub target: RouteTarget,
}

impl RouteTarget {
    pub fn for_view(schema: &Schema, view_name: &str) -> Self {
        match schema.target(view_name) {
            Target::Component(Component::Dashboard(id)) => {
                RouteTarget::Dashboard(id.map(Into::into))
            }
            Target::Component(Component::LiveDelivery) => RouteTarget::LiveDelivery,
            Target::Component(Component::LiveTracing) => RouteTarget::LiveTracing,
            Target::Component(Component::Other(_)) | Target::Unknown => {
                RouteTarget::Unknown(view_name.into())
            }
            Target::Object(view) => match view.kind() {
                ViewKind::Singleton => RouteTarget::Singleton(view.view.into()),
                ViewKind::CreateOnly => RouteTarget::Action(view.view.into()),
                ViewKind::List => RouteTarget::List(view.view.into()),
            },
        }
    }
}

impl Route {
    pub fn new(layout: Option<u8>, target: RouteTarget) -> Self {
        Route { layout, target }
    }

    pub fn view_name(&self) -> Option<&str> {
        match &self.target {
            RouteTarget::List(view)
            | RouteTarget::Query { view, .. }
            | RouteTarget::Object { view, .. }
            | RouteTarget::New(view)
            | RouteTarget::Singleton(view)
            | RouteTarget::Action(view) => Some(view),
            _ => None,
        }
    }

    pub fn link(&self) -> Option<Cow<'_, str>> {
        match &self.target {
            RouteTarget::Dashboard(None) => Some(Cow::Borrowed(DASHBOARD_LINK)),
            RouteTarget::Dashboard(Some(id)) => {
                let mut link = String::with_capacity(DASHBOARD_PREFIX.len() + id.len());
                link.push_str(DASHBOARD_PREFIX);
                link.push_str(id);
                Some(Cow::Owned(link))
            }
            RouteTarget::LiveDelivery => Some(Cow::Borrowed(DELIVERY_LINK)),
            RouteTarget::LiveTracing => Some(Cow::Borrowed(TRACING_LINK)),
            RouteTarget::Upsell(view) => Some(Cow::Borrowed(view)),
            _ => self.view_name().map(Cow::Borrowed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    Open,
    Back,
    Reload,
}

#[derive(Default)]
pub struct Router {
    current: Option<Route>,
    history: VecDeque<Route>,
    pending: Option<(Route, Arrival)>,
}

impl Router {
    pub fn current(&self) -> Option<&Route> {
        self.current.as_ref()
    }

    pub fn open(&mut self, route: Route) {
        if self.current.as_ref() == Some(&route) {
            return;
        }
        if let Some(previous) = self.current.take() {
            if self.history.len() == MAX_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(previous);
        }
        self.current = Some(route.clone());
        self.pending = Some((route, Arrival::Open));
    }

    pub fn jump(&mut self, route: Route) {
        if self.current.as_ref() == Some(&route) {
            return;
        }
        self.history.clear();
        self.current = Some(route.clone());
        self.pending = Some((route, Arrival::Open));
    }

    pub fn previous(&self) -> Option<&Route> {
        self.history.back()
    }

    pub fn depth(&self) -> usize {
        self.history.len()
    }

    pub fn replace(&mut self, route: Route) {
        self.current = Some(route.clone());
        self.pending = Some((route, Arrival::Open));
    }

    pub fn reload(&mut self) {
        self.pending = self.current.clone().map(|route| (route, Arrival::Reload));
    }

    pub fn back(&mut self) -> bool {
        match self.history.pop_back() {
            Some(route) => {
                self.current = Some(route.clone());
                self.pending = Some((route, Arrival::Back));
                true
            }
            None => false,
        }
    }

    pub fn take_pending(&mut self) -> Option<Route> {
        self.take_arrival().map(|(route, _)| route)
    }

    pub fn take_arrival(&mut self) -> Option<(Route, Arrival)> {
        self.pending.take()
    }

    pub fn reset(&mut self) {
        self.current = None;
        self.history.clear();
        self.pending = None;
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/tui/route/tests.rs"]
mod tests;
