/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::app::error::{CliError, CliResult};
use crate::schema::{FieldType, MapValueType, ObjectSchema, ObjectType, ScalarType, Schema};

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
    if let Some(name) = resolve_object(schema, raw, false)? {
        return Ok(name);
    }
    if let Some(canonical_embedded) = resolve_embedded(schema, raw) {
        let display = display_name(canonical_embedded).to_string();
        let holders = top_level_holders_of(schema, canonical_embedded);
        let msg = match holders.as_slice() {
            [] => format!("`{display}` is an embedded sub-object, not a top-level object"),
            [single] => {
                let tail = first_holder_field(schema, single, canonical_embedded)
                    .map(|f| format!("its {f}"))
                    .unwrap_or_else(|| "it".to_string());
                format!(
                    "`{display}` is an embedded property of {single}, not a separate object. \
                     Snapshot {single} to capture {tail}."
                )
            }
            _ => {
                let joined = join_or(&holders);
                format!(
                    "`{display}` is an embedded property of {joined}, not a separate object. \
                     Snapshot {joined} to capture it."
                )
            }
        };
        return Err(CliError::msg(msg));
    }
    Err(CliError::UnknownObject(display_name(raw).to_string()))
}

fn join_or(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} or {b}"),
        _ => {
            let mut out = String::new();
            for (i, name) in items.iter().enumerate() {
                if i + 1 == items.len() {
                    out.push_str(", or ");
                } else if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
            }
            out
        }
    }
}

fn first_holder_field<'a>(
    schema: &'a Schema,
    holder_display: &str,
    embedded: &str,
) -> Option<&'a str> {
    let holder_canonical = schema
        .objects
        .keys()
        .find(|k| display_name(k).eq_ignore_ascii_case(holder_display))?;
    let obj_schema = schema.schemas.get(holder_canonical)?;
    let schema_names: Vec<&str> = match obj_schema {
        ObjectSchema::Single { schema_name } => vec![schema_name.as_str()],
        ObjectSchema::Multiple { variants } => variants
            .iter()
            .filter_map(|v| v.schema_name.as_deref())
            .collect(),
    };
    let mut candidates: Vec<&str> = Vec::new();
    for schema_name in schema_names {
        let Some(fields) = schema.fields.get(schema_name) else {
            continue;
        };
        for (fname, field) in &fields.properties {
            if field_type_refs_embedded(&field.typ, embedded) {
                candidates.push(fname.as_str());
            }
        }
    }
    candidates.sort();
    candidates.into_iter().next()
}

fn resolve_embedded<'a>(schema: &'a Schema, raw: &str) -> Option<&'a str> {
    let candidate = canonicalise_with_prefix(raw);
    schema
        .schemas
        .keys()
        .find(|k| k.eq_ignore_ascii_case(&candidate))
        .map(String::as_str)
}

fn top_level_holders_of(schema: &Schema, embedded: &str) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (top_canonical, obj_type) in &schema.objects {
        if matches!(obj_type, ObjectType::View { .. }) {
            continue;
        }
        let Some(obj_schema) = schema.schemas.get(top_canonical) else {
            continue;
        };
        let referenced = match obj_schema {
            ObjectSchema::Single { schema_name } => {
                schema_refs_embedded(schema, schema_name, embedded)
            }
            ObjectSchema::Multiple { variants } => variants.iter().any(|v| {
                v.schema_name
                    .as_deref()
                    .map(|sn| schema_refs_embedded(schema, sn, embedded))
                    .unwrap_or(false)
            }),
        };
        if referenced {
            let display = display_name(top_canonical).to_string();
            if seen.insert(display.clone()) {
                hits.push(display);
            }
        }
    }
    hits.sort();
    hits
}

fn schema_refs_embedded(schema: &Schema, schema_name: &str, embedded: &str) -> bool {
    let Some(fields) = schema.fields.get(schema_name) else {
        return false;
    };
    for field in fields.properties.values() {
        if field_type_refs_embedded(&field.typ, embedded) {
            return true;
        }
    }
    false
}

fn field_type_refs_embedded(t: &FieldType, embedded: &str) -> bool {
    match t {
        FieldType::Object { object_name, .. } | FieldType::ObjectList { object_name, .. } => {
            object_name == embedded
        }
        FieldType::ObjectId { object_name, .. } => object_name == embedded,
        FieldType::Set { class, .. } => matches!(
            class,
            ScalarType::ObjectId { object_name } if object_name == embedded
        ),
        FieldType::Map {
            key_class,
            value_class,
            ..
        } => {
            let kc = matches!(
                key_class,
                ScalarType::ObjectId { object_name } if object_name == embedded
            );
            let vc = matches!(
                value_class,
                MapValueType::Object { object_name } if object_name == embedded
            );
            kc || vc
        }
        _ => false,
    }
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

    #[test]
    fn require_object_embedded_names_holder_object() {
        use crate::schema::{Field, FieldType, FieldUpdate, Fields, ObjectVariant};
        let mut s = schema_with(vec![("x:Account", obj()), ("x:AppPassword", obj())]);
        s.schemas.insert(
            "x:Credential".into(),
            ObjectSchema::Multiple {
                variants: vec![
                    ObjectVariant {
                        name: "Password".into(),
                        label: "".into(),
                        schema_name: Some("x:PasswordCredential".into()),
                    },
                    ObjectVariant {
                        name: "AppPassword".into(),
                        label: "".into(),
                        schema_name: Some("x:SecondaryCredential".into()),
                    },
                ],
            },
        );
        s.schemas.insert(
            "x:Account".into(),
            ObjectSchema::Multiple {
                variants: vec![ObjectVariant {
                    name: "User".into(),
                    label: "".into(),
                    schema_name: Some("x:UserAccount".into()),
                }],
            },
        );
        s.schemas.insert(
            "x:AppPassword".into(),
            ObjectSchema::Single {
                schema_name: "x:AppPassword".into(),
            },
        );
        let mut props: HashMap<String, Field> = HashMap::new();
        props.insert(
            "credentials".into(),
            Field {
                description: "".into(),
                typ: FieldType::ObjectList {
                    object_name: "x:Credential".into(),
                    min_items: None,
                    max_items: None,
                },
                update: FieldUpdate::Mutable,
                enterprise: false,
            },
        );
        s.fields.insert(
            "x:UserAccount".into(),
            Fields {
                properties: props,
                defaults: HashMap::new(),
            },
        );

        let err = require_object(&s, "Credential").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("embedded property of Account"),
            "expected `embedded property of Account` in: {msg}"
        );
        assert!(
            msg.contains("Snapshot Account"),
            "expected `Snapshot Account` in: {msg}"
        );
        assert!(
            msg.contains("credentials"),
            "expected single-holder message to mention the field name `credentials`: {msg}"
        );
        assert!(
            !msg.contains("Did you mean"),
            "must not include `Did you mean`: {msg}"
        );
        for sibling in ["AccountPassword", "AppPassword", "ApiKey"] {
            assert!(
                !msg.contains(sibling),
                "must not surface sibling embedded/per-user type `{sibling}`: {msg}"
            );
        }
    }

    #[test]
    fn require_object_embedded_lists_multiple_holders() {
        use crate::schema::{Field, FieldType, FieldUpdate, Fields, ObjectVariant};
        let mut s = schema_with(vec![("x:Account", obj()), ("x:Tenant", obj())]);
        s.schemas.insert(
            "x:Roles".into(),
            ObjectSchema::Multiple {
                variants: vec![ObjectVariant {
                    name: "Default".into(),
                    label: "".into(),
                    schema_name: Some("x:DefaultRoles".into()),
                }],
            },
        );
        s.schemas.insert(
            "x:Account".into(),
            ObjectSchema::Multiple {
                variants: vec![ObjectVariant {
                    name: "Group".into(),
                    label: "".into(),
                    schema_name: Some("x:GroupAccount".into()),
                }],
            },
        );
        s.schemas.insert(
            "x:Tenant".into(),
            ObjectSchema::Single {
                schema_name: "x:Tenant".into(),
            },
        );
        let roles_field = || Field {
            description: "".into(),
            typ: FieldType::Object {
                object_name: "x:Roles".into(),
                nullable: false,
            },
            update: FieldUpdate::Mutable,
            enterprise: false,
        };
        let mut group_props: HashMap<String, Field> = HashMap::new();
        group_props.insert("roles".into(), roles_field());
        s.fields.insert(
            "x:GroupAccount".into(),
            Fields {
                properties: group_props,
                defaults: HashMap::new(),
            },
        );
        let mut tenant_props: HashMap<String, Field> = HashMap::new();
        tenant_props.insert("roles".into(), roles_field());
        s.fields.insert(
            "x:Tenant".into(),
            Fields {
                properties: tenant_props,
                defaults: HashMap::new(),
            },
        );

        let err = require_object(&s, "Roles").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("embedded property of Account or Tenant"),
            "expected joined holders in: {msg}"
        );
        assert!(
            msg.contains("Snapshot Account or Tenant"),
            "expected snapshot guidance in: {msg}"
        );
        assert!(
            !msg.contains("Did you mean"),
            "must not include `Did you mean`: {msg}"
        );
    }
}
