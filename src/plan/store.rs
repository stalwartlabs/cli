/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::Result;
use crate::jmap::Client;
use crate::jmap::pages::Fetch;
use crate::plan::refs::ResolvedScope;
use crate::plan::{AT_TYPE, ID};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub type Object = Map<String, Value>;

pub fn object_id(object: &Object) -> Option<&str> {
    object.get(ID).and_then(Value::as_str)
}

#[derive(Default)]
struct Cached {
    objects: Vec<Object>,
    by_id: Option<HashMap<String, usize>>,
}

impl Cached {
    fn push(&mut self, object: Object) {
        if let Some(by_id) = &mut self.by_id
            && let Some(id) = object_id(&object)
        {
            by_id.entry(id.to_string()).or_insert(self.objects.len());
        }
        self.objects.push(object);
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Object> {
        let objects = &self.objects;
        let by_id = self.by_id.get_or_insert_with(|| {
            let mut by_id = HashMap::with_capacity(objects.len());
            for (index, object) in objects.iter().enumerate() {
                if let Some(id) = object_id(object) {
                    by_id.entry(id.to_string()).or_insert(index);
                }
            }
            by_id
        });
        by_id.get(id).and_then(|&index| self.objects.get_mut(index))
    }
}

#[derive(Default)]
pub struct ObjectStore<'s> {
    types: HashMap<&'s str, Cached>,
}

impl<'s> ObjectStore<'s> {
    pub fn contains(&self, object: &str) -> bool {
        self.types.contains_key(object)
    }

    pub fn fetch(&mut self, client: &Client, object: &'s str) -> Result<usize> {
        let mut objects = Vec::new();
        for page in client.pages(object, None, Fetch::Objects { properties: None }) {
            objects.extend(page?.objects);
        }
        let count = objects.len();
        self.insert(object, objects);
        Ok(count)
    }

    pub fn ensure(&mut self, client: &Client, object: &'s str) -> Result<()> {
        if !self.contains(object) {
            self.fetch(client, object)?;
        }
        Ok(())
    }

    pub fn insert(&mut self, object: &'s str, objects: Vec<Object>) {
        self.types.insert(
            object,
            Cached {
                objects,
                by_id: None,
            },
        );
    }

    pub fn objects_for(&self, object: &str) -> &[Object] {
        self.types
            .get(object)
            .map_or(&[], |cached| cached.objects.as_slice())
    }

    pub fn partition<'a>(
        &'a self,
        object: &str,
        variant: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Object> {
        self.objects_for(object)
            .iter()
            .filter(move |o| o.get(AT_TYPE).and_then(Value::as_str) == variant)
    }

    pub fn record_created(&mut self, object: &str, objects: impl ExactSizeIterator<Item = Object>) {
        if let Some(cached) = self.types.get_mut(object) {
            cached.objects.reserve(objects.len());
            for o in objects {
                cached.push(o);
            }
        }
    }

    pub fn record_updated(&mut self, object: &str, id: &str, patch: &Object) {
        let Some(obj) = self
            .types
            .get_mut(object)
            .and_then(|cached| cached.get_mut(id))
        else {
            return;
        };
        for (prop, value) in patch {
            if prop.contains('/') {
                continue;
            }
            if value.is_null() {
                obj.remove(prop);
            } else {
                obj.insert(prop.clone(), value.clone());
            }
        }
    }

    pub fn invalidate(&mut self, object: &str) {
        self.types.remove(object);
    }

    pub fn leaked(
        &self,
        object: &str,
        matched: &HashSet<String>,
        scope: Option<&ResolvedScope<'_>>,
    ) -> Vec<String> {
        self.objects_for(object)
            .iter()
            .filter_map(|cand| {
                let id = object_id(cand)?;
                (!matched.contains(id) && scope.is_none_or(|s| s.contains(cand)))
                    .then(|| id.to_string())
            })
            .collect()
    }
}
