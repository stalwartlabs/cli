/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{
    AT_TYPE, EnumVariant, Field, Fields, List, ObjectSchema, ObjectType, ObjectVariant, Schema,
};
use serde_json::{Map, Value};

const SINGLETON_ID: &str = "singleton";

#[derive(Clone, Copy, Debug)]
pub struct ObjectDef<'a> {
    pub(super) schema: &'a Schema,
    name: &'a str,
    pub(super) kind: &'a ObjectType,
    pub(super) shape: Option<&'a ObjectSchema>,
}

#[derive(Clone, Copy, Debug)]
pub enum FieldSet<'a> {
    Empty,
    One(&'a Fields),
    Union {
        schema: &'a Schema,
        variants: &'a [ObjectVariant],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectId<'a> {
    Singleton,
    Id(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantError<'o> {
    Missing,
    Unknown(&'o str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdError {
    BadSingleton,
    Missing,
}

pub fn display_name(canonical: &str) -> &str {
    canonical.strip_prefix("x:").unwrap_or(canonical)
}

pub fn split_view(object_name: &str) -> (&str, Option<&str>) {
    match object_name.split_once('/') {
        Some((base, variant)) => (base, Some(variant)),
        None => (object_name, None),
    }
}

impl Schema {
    pub fn get_object(&self, canonical: &str) -> Option<ObjectDef<'_>> {
        let (name, kind) = self.objects.get_key_value(canonical)?;
        Some(ObjectDef {
            schema: self,
            name,
            kind,
            shape: self.schemas.get(canonical),
        })
    }

    pub fn enum_ci(&self, raw: &str) -> Option<(&str, &[EnumVariant])> {
        find_ci(self.enums.get_key_value(raw), &self.enums, raw)
            .map(|(name, variants)| (name, variants.as_slice()))
    }

    pub fn enum_variant(&self, enum_name: &str, name: &str) -> Option<&EnumVariant> {
        self.enums.get(enum_name)?.iter().find(|v| v.name == name)
    }

    pub fn views_of<'a>(&'a self, parent: &'a str) -> impl Iterator<Item = &'a str> {
        self.objects
            .iter()
            .filter_map(move |(key, kind)| match kind {
                ObjectType::View { object_name } if object_name == parent => Some(key.as_str()),
                _ => None,
            })
    }

    fn fields_named(&self, schema_name: Option<&str>) -> Option<&Fields> {
        self.fields.get(schema_name?)
    }

    pub(super) fn object_ci(&self, raw: &str) -> Option<(&str, &ObjectType)> {
        find_ci(self.objects.get_key_value(raw), &self.objects, raw)
    }

    pub(super) fn schema_name_ci(&self, raw: &str) -> Option<&str> {
        find_ci(self.schemas.get_key_value(raw), &self.schemas, raw).map(|(name, _)| name)
    }
}

fn find_ci<'m, V: 'm>(
    exact: Option<(&'m String, &'m V)>,
    entries: impl IntoIterator<Item = (&'m String, &'m V)>,
    raw: &str,
) -> Option<(&'m str, &'m V)> {
    exact
        .or_else(|| {
            entries
                .into_iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(raw))
        })
        .map(|(key, value)| (key.as_str(), value))
}

impl<'a> ObjectDef<'a> {
    pub fn name(self) -> &'a str {
        self.name
    }

    pub fn display_name(self) -> &'a str {
        display_name(self.name)
    }

    pub fn permission_prefix(self) -> Option<&'a str> {
        match self.kind {
            ObjectType::Singleton {
                permission_prefix, ..
            }
            | ObjectType::Object {
                permission_prefix, ..
            } => Some(permission_prefix),
            ObjectType::View { .. } => None,
        }
    }

    pub fn is_enterprise(self) -> bool {
        matches!(
            self.kind,
            ObjectType::Singleton {
                enterprise: true,
                ..
            } | ObjectType::Object {
                enterprise: true,
                ..
            }
        )
    }

    pub fn is_singleton(self) -> bool {
        matches!(self.kind, ObjectType::Singleton { .. })
    }

    pub fn is_multi_variant(self) -> bool {
        matches!(self.shape, Some(ObjectSchema::Multiple { .. }))
    }

    pub fn variants(self) -> &'a [ObjectVariant] {
        match self.shape {
            Some(ObjectSchema::Multiple { variants }) => variants,
            _ => &[],
        }
    }

    pub fn variant(self, name: &str) -> Option<&'a ObjectVariant> {
        self.variants().iter().find(|v| v.name == name)
    }

    pub fn variant_ci(self, name: &str) -> Option<&'a ObjectVariant> {
        let variants = self.variants();
        variants
            .iter()
            .find(|v| v.name == name)
            .or_else(|| variants.iter().find(|v| v.name.eq_ignore_ascii_case(name)))
    }

    pub fn fields_for(self, at_type: Option<&str>) -> Option<&'a Fields> {
        match self.shape? {
            ObjectSchema::Single { schema_name } => self.schema.fields.get(schema_name),
            ObjectSchema::Multiple { .. } => {
                let variant = self.variant(at_type?)?;
                self.schema.fields_named(variant.schema_name.as_deref())
            }
        }
    }

    pub fn fields_of(self, obj: &Map<String, Value>) -> Option<&'a Fields> {
        let shape = self.shape?;
        match shape.single_name() {
            Some(schema_name) => self.schema.fields.get(schema_name),
            None => self
                .schema
                .fields_named(shape.variant_of(obj)?.schema_name.as_deref()),
        }
    }

    pub fn typed_fields<'o>(
        self,
        obj: &'o Map<String, Value>,
    ) -> Result<Option<&'a Fields>, VariantError<'o>> {
        if !self.is_multi_variant() {
            return Ok(self.fields_for(None));
        }
        let at_type = obj
            .get(AT_TYPE)
            .and_then(Value::as_str)
            .ok_or(VariantError::Missing)?;
        let variant = self
            .variant(at_type)
            .ok_or(VariantError::Unknown(at_type))?;
        Ok(self.schema.fields_named(variant.schema_name.as_deref()))
    }

    pub fn variant_fields(self, variant: &ObjectVariant) -> FieldSet<'a> {
        self.schema
            .fields_named(variant.schema_name.as_deref())
            .map_or(FieldSet::Empty, FieldSet::One)
    }

    pub fn all_fields(self) -> FieldSet<'a> {
        match self.shape {
            Some(ObjectSchema::Single { schema_name }) => self
                .schema
                .fields
                .get(schema_name)
                .map_or(FieldSet::Empty, FieldSet::One),
            Some(ObjectSchema::Multiple { variants }) => FieldSet::Union {
                schema: self.schema,
                variants,
            },
            None => FieldSet::Empty,
        }
    }

    pub fn label_property(self) -> Option<&'a str> {
        self.schema.lists.get(self.name)?.label_property.as_deref()
    }

    pub fn list(self) -> Option<&'a List> {
        let lists = &self.schema.lists;
        lists.get(self.name).or_else(|| {
            self.schema
                .views_of(self.name)
                .find_map(|view| lists.get(view))
        })
    }

    pub fn object_id(self, raw: Option<&'a str>) -> Result<ObjectId<'a>, IdError> {
        match (self.is_singleton(), raw) {
            (true, None | Some(SINGLETON_ID)) => Ok(ObjectId::Singleton),
            (true, Some(_)) => Err(IdError::BadSingleton),
            (false, Some(id)) if !id.is_empty() => Ok(ObjectId::Id(id)),
            (false, _) => Err(IdError::Missing),
        }
    }
}

impl<'a> FieldSet<'a> {
    pub fn get(self, name: &str) -> Option<&'a Field> {
        self.sets().find_map(|fields| fields.properties.get(name))
    }

    pub fn get_ci(self, name: &str) -> Option<(&'a str, &'a Field)> {
        self.sets()
            .find_map(|fields| fields.properties.get_key_value(name))
            .or_else(|| {
                self.sets().find_map(|fields| {
                    fields
                        .properties
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                })
            })
            .map(|(k, f)| (k.as_str(), f))
    }

    pub fn defs(self, name: &str) -> impl Iterator<Item = &'a Field> {
        self.sets()
            .filter_map(move |fields| fields.properties.get(name))
    }

    pub fn all_defs(self) -> impl Iterator<Item = (&'a str, &'a Field)> {
        self.sets()
            .flat_map(|fields| &fields.properties)
            .map(|(name, field)| (name.as_str(), field))
    }

    pub fn is_empty(self) -> bool {
        self.sets().all(|fields| fields.properties.is_empty())
    }

    fn sets(self) -> impl Iterator<Item = &'a Fields> {
        let (one, union) = match self {
            FieldSet::Empty => (None, None),
            FieldSet::One(fields) => (Some(fields), None),
            FieldSet::Union { schema, variants } => (None, Some((schema, variants))),
        };
        one.into_iter()
            .chain(union.into_iter().flat_map(|(schema, variants)| {
                variants
                    .iter()
                    .filter_map(|v| schema.fields_named(v.schema_name.as_deref()))
            }))
    }
}

impl<'a> ObjectId<'a> {
    pub fn as_str(self) -> &'a str {
        match self {
            ObjectId::Singleton => SINGLETON_ID,
            ObjectId::Id(id) => id,
        }
    }
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/object/tests.rs"]
mod tests;

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/object/access_tests.rs"]
mod access_tests;
