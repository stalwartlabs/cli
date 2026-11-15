/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::error::{Error, Result};
use crate::schema::object::display_name;
use crate::schema::{
    EmbeddedObject, ObjectDef, ObjectSchema, ObjectType, ObjectVariant, ResolveError, Schema,
};

const OBJECT_PREFIX: &str = "x:";

#[derive(Clone, Copy)]
enum ViewPolicy {
    Reject,
    NotFound,
}

impl Schema {
    pub fn object(&self, raw: &str) -> Result<ObjectDef<'_>> {
        if let Some(def) = self.lookup_object(raw, ViewPolicy::Reject)? {
            return Ok(def);
        }
        Err(self.embedded_or_unknown(raw).into())
    }

    pub fn object_for_describe(&self, raw: &str) -> Result<Option<ObjectDef<'_>>> {
        self.lookup_object(raw, ViewPolicy::NotFound)
    }

    pub fn object_no_variant(&self, raw: &str) -> Result<ObjectDef<'_>> {
        if raw.contains('/') {
            return Err(ResolveError::VariantOutsideCreate.into());
        }
        self.object(raw)
    }

    pub fn create_target(&self, raw: &str) -> Result<(ObjectDef<'_>, Option<&ObjectVariant>)> {
        let (obj_part, variant_part) = match raw.split_once('/') {
            Some((o, v)) => (o, Some(v)),
            None => (raw, None),
        };
        let def = self.object(obj_part)?;
        let Some(wanted) = variant_part else {
            return Ok((def, None));
        };
        match def.shape {
            None => Err(Error::UnexpectedResponse(format!(
                "schema missing for {}",
                def.name()
            ))),
            Some(ObjectSchema::Single { .. }) => {
                Err(ResolveError::NotMultiVariant(def.display_name().to_string()).into())
            }
            Some(ObjectSchema::Multiple { .. }) => match def.variant_ci(wanted) {
                Some(variant) => Ok((def, Some(variant))),
                None => Err(ResolveError::UnknownVariant {
                    object: def.display_name().to_string(),
                    variant: wanted.to_string(),
                }
                .into()),
            },
        }
    }

    fn lookup_object(&self, raw: &str, views: ViewPolicy) -> Result<Option<ObjectDef<'_>>> {
        let candidate = canonicalise_with_prefix(raw);
        let Some((key, kind)) = self.object_ci(&candidate) else {
            return Ok(None);
        };
        match (kind, views) {
            (ObjectType::View { .. }, ViewPolicy::NotFound) => Ok(None),
            (ObjectType::View { .. }, ViewPolicy::Reject) => {
                Err(ResolveError::ObjectIsView(display_name(key).to_string()).into())
            }
            _ => Ok(self.get_object(key)),
        }
    }

    fn embedded_or_unknown(&self, raw: &str) -> ResolveError {
        let Some(embedded) = self.schema_name_ci(&canonicalise_with_prefix(raw)) else {
            return ResolveError::UnknownObject(display_name(raw).to_string());
        };
        let mut holders: Vec<ObjectDef<'_>> = self.top_level_holders_of(embedded).collect();
        holders.sort_unstable_by_key(|def| def.display_name());
        holders.dedup_by_key(|def| def.display_name());
        let field = match holders.as_slice() {
            [single] => single
                .all_fields()
                .all_defs()
                .filter(|(_, field)| field.typ.references(embedded))
                .map(|(name, _)| name)
                .min(),
            _ => None,
        };
        ResolveError::Embedded(EmbeddedObject {
            name: display_name(embedded).to_string(),
            holders: holders
                .into_iter()
                .map(|def| def.display_name().to_string())
                .collect(),
            field: field.map(str::to_string),
        })
    }

    fn top_level_holders_of<'a>(
        &'a self,
        embedded: &'a str,
    ) -> impl Iterator<Item = ObjectDef<'a>> {
        self.objects
            .iter()
            .filter(|(_, kind)| !matches!(kind, ObjectType::View { .. }))
            .filter_map(|(canonical, _)| self.get_object(canonical))
            .filter(move |def| {
                def.all_fields()
                    .all_defs()
                    .any(|(_, field)| field.typ.references(embedded))
            })
    }
}

fn canonicalise_with_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    let bare = match trimmed.split_at_checked(OBJECT_PREFIX.len()) {
        Some((prefix, rest)) if prefix.eq_ignore_ascii_case(OBJECT_PREFIX) => rest,
        _ => trimmed,
    };
    let mut out = String::with_capacity(OBJECT_PREFIX.len() + bare.len());
    out.push_str(OBJECT_PREFIX);
    out.push_str(bare);
    out
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/resolve/tests.rs"]
mod tests;
