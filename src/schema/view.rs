/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::schema::{
    AT_TYPE, Field, Fields, Form, FormField, List, ObjectDef, ObjectSchema, ObjectType,
    ObjectVariant, Schema,
};
use serde_json::Value;
use std::collections::HashMap;

const COMPONENT_PREFIX: &str = "CustomComponent/";
const DASHBOARD_PREFIX: &str = "Dashboard/";
const NAME_PROPERTY: &str = "name";
const PERMISSION_ENUM: &str = "Permission";
const CREATE_SUFFIX: &str = "Create";
const GET_SUFFIX: &str = "Get";

#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Component(Component<'a>),
    Object(ViewDef<'a>),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component<'a> {
    Dashboard(Option<&'a str>),
    LiveDelivery,
    LiveTracing,
    Other(&'a str),
}

#[derive(Clone, Copy, Debug)]
pub struct ViewDef<'a> {
    pub view: &'a str,
    pub object: ObjectDef<'a>,
    pub list: Option<&'a List>,
    pub form: Option<&'a Form>,
    pub static_filter: Option<&'a HashMap<String, Value>>,
    pub variant: Option<&'a ObjectVariant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Singleton,
    List,
    CreateOnly,
}

#[derive(Debug, Default)]
pub struct FormSections<'a> {
    pub title: Option<&'a str>,
    pub sections: Vec<SectionView<'a>>,
}

#[derive(Debug)]
pub struct SectionView<'a> {
    pub title: Option<&'a str>,
    pub fields: Vec<FieldEntry<'a>>,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldEntry<'a> {
    pub name: &'a str,
    pub label: &'a str,
    pub form_field: Option<&'a FormField>,
    pub field: Option<&'a Field>,
}

impl Schema {
    pub fn target<'a>(&'a self, view_name: &'a str) -> Target<'a> {
        if let Some(name) = view_name.strip_prefix(COMPONENT_PREFIX) {
            let component = hashify::fnc_map!(name.as_bytes(),
                "Dashboard" => Component::Dashboard(None),
                "LiveDelivery" => Component::LiveDelivery,
                "LiveTracing" => Component::LiveTracing,
                _ => Component::Other(name)
            );
            return Target::Component(component);
        }
        if let Some(id) = view_name.strip_prefix(DASHBOARD_PREFIX) {
            return Target::Component(Component::Dashboard(Some(id)));
        }
        match self.view(view_name) {
            Some(view) => Target::Object(view),
            None => Target::Unknown,
        }
    }

    pub fn is_create_only(&self, permission_prefix: &str) -> bool {
        let mut create = false;
        for variant in self.enums.get(PERMISSION_ENUM).into_iter().flatten() {
            match variant.name.strip_prefix(permission_prefix) {
                Some(GET_SUFFIX) => return false,
                Some(CREATE_SUFFIX) => create = true,
                _ => {}
            }
        }
        create
    }

    pub fn embedded_form_sections(
        &self,
        schema_name: &str,
        at_type: Option<&str>,
    ) -> FormSections<'_> {
        let own_form = self.forms.get(schema_name);
        match self.schemas.get(schema_name) {
            Some(ObjectSchema::Single { schema_name: inner }) => FormSections::build(
                own_form.or_else(|| self.forms.get(inner)),
                None,
                self.fields.get(inner),
            ),
            Some(ObjectSchema::Multiple { variants }) => {
                let inner = at_type
                    .and_then(|name| variants.iter().find(|v| v.name == name))
                    .and_then(|v| v.schema_name.as_deref());
                FormSections::build(
                    own_form,
                    inner.and_then(|name| self.forms.get(name)),
                    inner.and_then(|name| self.fields.get(name)),
                )
            }
            None => FormSections::build(own_form, None, self.fields.get(schema_name)),
        }
    }

    pub fn view(&self, view_name: &str) -> Option<ViewDef<'_>> {
        let (view, kind) = self.objects.get_key_value(view_name)?;
        let object_name = match kind {
            ObjectType::View { object_name } => object_name.as_str(),
            _ => view.as_str(),
        };
        let object = self.get_object(object_name)?;
        if matches!(object.kind, ObjectType::View { .. }) {
            return None;
        }
        let shape = object.shape?;
        let list = self
            .lists
            .get(view.as_str())
            .or_else(|| self.lists.get(object_name));
        let form = self
            .forms
            .get(view.as_str())
            .or_else(|| self.forms.get(object_name))
            .or_else(|| self.forms.get(shape.single_name()?));
        let static_filter = list
            .map(|list| &list.filters_static)
            .filter(|filter| !filter.is_empty());
        let variant = static_filter
            .and_then(|filter| filter.get(AT_TYPE)?.as_str())
            .and_then(|name| object.variant(name));
        Some(ViewDef {
            view,
            object,
            list,
            form,
            static_filter,
            variant,
        })
    }
}

impl<'a> ViewDef<'a> {
    pub fn kind(&self) -> ViewKind {
        if self.object.is_singleton() {
            ViewKind::Singleton
        } else if self
            .object
            .permission_prefix()
            .is_some_and(|prefix| self.object.schema.is_create_only(prefix))
        {
            ViewKind::CreateOnly
        } else {
            ViewKind::List
        }
    }

    pub fn label_property(&self) -> Option<&'a str> {
        let schema = self.object.schema;
        let object = self.object.name();
        list_label(self.list)
            .or_else(|| list_label(schema.lists.get(object)))
            .or_else(|| {
                schema
                    .views_of(object)
                    .filter(|view| *view != self.view)
                    .find_map(|view| list_label(schema.lists.get(view)))
            })
            .or_else(|| {
                self.object
                    .all_fields()
                    .get(NAME_PROPERTY)
                    .map(|_| NAME_PROPERTY)
            })
    }

    pub fn form_sections(&self, at_type: Option<&str>) -> FormSections<'a> {
        let schema = self.object.schema;
        let (form, variant_form, fields) = match self.object.shape {
            Some(ObjectSchema::Multiple { .. }) => {
                let variant = at_type.and_then(|name| self.object.variant(name));
                let schema_name = variant.and_then(|v| v.schema_name.as_deref());
                (
                    self.form,
                    schema_name.and_then(|name| schema.forms.get(name)),
                    schema_name.and_then(|name| schema.fields.get(name)),
                )
            }
            _ => (self.form, None, self.object.fields_for(None)),
        };
        FormSections::build(form, variant_form, fields)
    }
}

impl<'a> FormSections<'a> {
    fn build(
        form: Option<&'a Form>,
        variant_form: Option<&'a Form>,
        fields: Option<&'a Fields>,
    ) -> Self {
        let mut out = FormSections {
            title: form.or(variant_form).and_then(|f| f.title.as_deref()),
            sections: Vec::new(),
        };
        let declared = form
            .into_iter()
            .chain(variant_form)
            .flat_map(|f| &f.sections);
        for section in declared {
            let entries: Vec<FieldEntry<'a>> = section
                .fields
                .iter()
                .filter_map(|ff| FieldEntry::declared(ff, fields))
                .collect();
            if !entries.is_empty() {
                out.sections.push(SectionView {
                    title: section.title.as_deref(),
                    fields: entries,
                });
            }
        }
        if form.is_none() && variant_form.is_none() {
            let entries: Vec<FieldEntry<'a>> = fields
                .into_iter()
                .flat_map(|f| &f.properties)
                .map(|(name, field)| FieldEntry {
                    name,
                    label: name,
                    form_field: None,
                    field: Some(field),
                })
                .collect();
            if !entries.is_empty() {
                out.sections.push(SectionView {
                    title: None,
                    fields: entries,
                });
            }
        }
        out
    }
}

impl<'a> FieldEntry<'a> {
    fn declared(form_field: &'a FormField, fields: Option<&'a Fields>) -> Option<Self> {
        let field = if form_field.name == AT_TYPE {
            None
        } else {
            Some(fields?.properties.get(&form_field.name)?)
        };
        Some(FieldEntry {
            name: &form_field.name,
            label: form_field.label_or_name(),
            form_field: Some(form_field),
            field,
        })
    }
}

fn list_label(list: Option<&List>) -> Option<&str> {
    let list = list?;
    list.label_property
        .as_deref()
        .filter(|label| !label.is_empty())
        .or_else(|| list.columns.first().map(|column| column.name.as_str()))
        .filter(|label| !label.is_empty())
}

#[cfg(all(test, stalwart_cli_tests))]
#[rustfmt::skip]
#[path = "../../../cli-tests/unit/src/schema/view/tests.rs"]
mod tests;
