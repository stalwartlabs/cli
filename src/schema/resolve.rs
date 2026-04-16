/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::schema::{ObjectSchema, ObjectType, Schema};

pub fn resolve_object<'a>(
    schema: &'a Schema,
    raw: &str,
    views_as_not_found: bool,
) -> CliResult<Option<&'a str>> {
    let candidate = canonicalise_with_prefix(raw);
    let matched = schema
        .objects
        .keys()
        .find(|k| k.eq_ignore_ascii_case(&candidate));

    let key = match matched {
        Some(k) => k,
        None => return Ok(None),
    };

    match &schema.objects[key] {
        ObjectType::View { .. } => {
            if views_as_not_found {
                Ok(None)
            } else {
                Err(CliError::ObjectIsView(display_name(key).to_string()))
            }
        }
        _ => Ok(Some(key.as_str())),
    }
}

pub fn require_object<'a>(schema: &'a Schema, raw: &str) -> CliResult<&'a str> {
    resolve_object(schema, raw, false)?
        .ok_or_else(|| CliError::UnknownObject(display_name(raw).to_string()))
}

pub fn forbid_slash_form(raw: &str) -> CliResult<()> {
    if raw.contains('/') {
        return Err(CliError::msg(
            "the Object/Variant syntax is only valid for `create`",
        ));
    }
    Ok(())
}

pub fn resolve_create_target<'a>(
    schema: &'a Schema,
    raw: &str,
) -> CliResult<(&'a str, Option<String>)> {
    let (obj_part, variant_part) = match raw.split_once('/') {
        Some((o, v)) => (o, Some(v)),
        None => (raw, None),
    };

    let obj_key = require_object(schema, obj_part)?;

    let variant_canonical = match variant_part {
        None => None,
        Some(v) => {
            let obj_schema = schema.schemas.get(obj_key).ok_or_else(|| {
                CliError::UnexpectedResponse(format!("schema missing for {obj_key}"))
            })?;
            match obj_schema {
                ObjectSchema::Single { .. } => {
                    return Err(CliError::NotMultiVariant(display_name(obj_key).to_string()));
                }
                ObjectSchema::Multiple { variants } => {
                    let found = variants.iter().find(|var| var.name.eq_ignore_ascii_case(v));
                    match found {
                        Some(var) => Some(var.name.clone()),
                        None => {
                            return Err(CliError::UnknownVariant {
                                object: display_name(obj_key).to_string(),
                                variant: v.to_string(),
                            });
                        }
                    }
                }
            }
        }
    };

    Ok((obj_key, variant_canonical))
}

pub fn resolve_enum<'a>(schema: &'a Schema, raw: &str) -> Option<&'a str> {
    schema
        .enums
        .keys()
        .find(|k| k.eq_ignore_ascii_case(raw))
        .map(|s| s.as_str())
}

pub fn canonicalise_with_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    let lower2 = trimmed
        .get(..2)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if lower2 == "x:" {
        let mut out = String::with_capacity(trimmed.len());
        out.push_str("x:");
        out.push_str(&trimmed[2..]);
        out
    } else {
        let mut out = String::with_capacity(trimmed.len() + 2);
        out.push_str("x:");
        out.push_str(trimmed);
        out
    }
}

pub fn display_name(canonical: &str) -> &str {
    canonical.strip_prefix("x:").unwrap_or(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;
    use std::collections::HashMap;

    fn schema_with(objs: Vec<(&str, ObjectType)>) -> Schema {
        let mut s = Schema::default();
        for (k, v) in objs {
            s.objects.insert(k.to_string(), v);
        }
        s
    }

    fn obj() -> ObjectType {
        ObjectType::Object {
            description: "".into(),
            permission_prefix: "".into(),
            enterprise: false,
        }
    }

    fn view(parent: &str) -> ObjectType {
        ObjectType::View {
            object_name: parent.into(),
        }
    }

    #[test]
    fn canonicalise_prepends_prefix() {
        assert_eq!(canonicalise_with_prefix("domain"), "x:domain");
        assert_eq!(canonicalise_with_prefix("X:Domain"), "x:Domain");
        assert_eq!(canonicalise_with_prefix("x:Domain"), "x:Domain");
    }

    #[test]
    fn resolve_case_insensitive() {
        let s = schema_with(vec![("x:Domain", obj())]);
        assert_eq!(
            resolve_object(&s, "domain", false).unwrap(),
            Some("x:Domain")
        );
        assert_eq!(
            resolve_object(&s, "DOMAIN", false).unwrap(),
            Some("x:Domain")
        );
        assert_eq!(
            resolve_object(&s, "x:domain", false).unwrap(),
            Some("x:Domain")
        );
    }

    #[test]
    fn resolve_unknown_is_none() {
        let s = schema_with(vec![("x:Domain", obj())]);
        assert!(resolve_object(&s, "nope", false).unwrap().is_none());
    }

    #[test]
    fn resolve_rejects_view_by_default() {
        let s = schema_with(vec![("x:Account/User", view("x:Account"))]);
        let err = resolve_object(&s, "account/user", false).unwrap_err();
        matches!(err, CliError::ObjectIsView(_));
    }

    #[test]
    fn resolve_views_as_not_found_for_describe() {
        let s = schema_with(vec![("x:Account/User", view("x:Account"))]);
        assert!(resolve_object(&s, "account/user", true).unwrap().is_none());
    }

    #[test]
    fn create_target_requires_multi_for_slash_form() {
        let mut s = schema_with(vec![("x:Domain", obj())]);
        s.schemas.insert(
            "x:Domain".into(),
            ObjectSchema::Single {
                schema_name: "x:Domain".into(),
            },
        );
        let err = resolve_create_target(&s, "domain/user").unwrap_err();
        matches!(err, CliError::NotMultiVariant(_));
    }

    #[test]
    fn display_name_strips_prefix() {
        assert_eq!(display_name("x:Domain"), "Domain");
        assert_eq!(display_name("Mailbox"), "Mailbox");
    }

    #[allow(dead_code)]
    fn _suppress_unused(schema: &Schema) -> Option<&str> {
        resolve_enum(schema, "foo")
    }

    fn _keep_hashmap_used(_h: HashMap<String, String>) {}
}
