//! Named provider settings and their retained public service/model revisions.
//! A slot owns its credential even when another slot uses the same upstream.
use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ProviderCatalog {
    default: Option<ProviderFields>,
    backup: Option<ProviderFields>,
    // Format 1/2 have no named execution history. Reading never rewrites them.
    history: Vec<RetainedProvider>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Slot {
    Default,
    Backup,
}
impl Slot {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "default" => Ok(Self::Default),
            "backup" => Ok(Self::Backup),
            _ => Err("model provider slot must be default or backup".into()),
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Backup => "backup",
        }
    }
    pub(super) fn credential_id(self) -> &'static str {
        match self {
            Self::Default => "slot:default",
            Self::Backup => "slot:backup",
        }
    }
    fn retained_credential_id(self, fields: &ProviderFields) -> String {
        format!(
            "{}:{}",
            self.credential_id(),
            fields.model_fields("").provider_id()
        )
    }
}
impl ProviderCatalog {
    pub(super) fn get(&self, slot: Slot) -> Option<&ProviderFields> {
        match slot {
            Slot::Default => self.default.as_ref(),
            Slot::Backup => self.backup.as_ref(),
        }
    }
    pub(super) fn history(&self, slot: Slot) -> impl Iterator<Item = &ProviderFields> {
        self.history
            .iter()
            .filter(move |retained| retained.slot == slot)
            .map(|retained| &retained.fields)
    }
    fn retain(&mut self, slot: Slot, fields: &ProviderFields) {
        if !fields.endpoint.is_empty()
            && fields.validate(None).is_ok()
            && !self
                .history
                .iter()
                .any(|retained| retained.slot == slot && &retained.fields == fields)
        {
            self.history.push(RetainedProvider {
                slot,
                fields: fields.clone(),
            });
        }
    }
    fn set(&mut self, slot: Slot, fields: ProviderFields) {
        match slot {
            Slot::Default => self.default = Some(fields),
            Slot::Backup => self.backup = Some(fields),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedProvider {
    slot: Slot,
    fields: ProviderFields,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ProviderFields {
    pub(super) endpoint: String,
    pub(super) wire_api: String,
    pub(super) flash: String,
    pub(super) pro: String,
}
impl Default for ProviderFields {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            wire_api: "responses".into(),
            flash: String::new(),
            pro: String::new(),
        }
    }
}
impl ProviderFields {
    pub(super) fn model_fields(&self, model: &str) -> ModelFields {
        ModelFields {
            endpoint: self.endpoint.clone(),
            wire_api: self.wire_api.clone(),
            model: model.into(),
            ..ModelFields::default()
        }
    }
    pub(super) fn same_service(&self, other: &Self) -> bool {
        !self.endpoint.is_empty()
            && !other.endpoint.is_empty()
            && self.model_fields("").provider_id() == other.model_fields("").provider_id()
    }
    fn registry(&self, model: &str, key: &str) -> Result<ModelRegistry, String> {
        ModelRegistry::from_fields(&self.model_fields(model), key, false)
    }
    fn validate(&self, key: Option<&str>) -> Result<(), String> {
        crate::goose_acp::configuration::wire_api(&self.wire_api)
            .map_err(|_| "wire_api must be chat or responses")?;
        if self.endpoint.is_empty() && self.flash.is_empty() && self.pro.is_empty() && key.is_none()
        {
            return Ok(());
        }
        let key = key.unwrap_or("validation-placeholder");
        self.registry("validation-placeholder", key)?;
        for model in [&self.flash, &self.pro] {
            if !model.is_empty() {
                self.registry(model, key)?;
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderUpdate {
    #[serde(flatten)]
    fields: ProviderFields,
    #[serde(default)]
    api_key: KeyUpdate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderTest {
    #[serde(flatten)]
    fields: ProviderFields,
    #[serde(default)]
    api_key: KeyUpdate,
    tier: Tier,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Tier {
    Flash,
    Pro,
}

impl ModelConfiguration {
    fn provider_fields(
        &self,
        managed: &ManagedModel,
        slot: Slot,
    ) -> Result<(ProviderFields, &'static str), String> {
        if let Some(fields) = managed.providers.get(slot) {
            return Ok((fields.clone(), "host"));
        }
        if matches!(slot, Slot::Default) {
            let legacy = self.effective(&managed.current)?;
            if !legacy.endpoint.is_empty() {
                return Ok((
                    ProviderFields {
                        endpoint: legacy.endpoint,
                        wire_api: legacy.wire_api,
                        flash: legacy.model,
                        pro: String::new(),
                    },
                    "legacy",
                ));
            }
        }
        Ok((ProviderFields::default(), "empty"))
    }
    pub(super) fn provider_key<'a>(
        &'a self,
        managed: &'a ManagedModel,
        slot: Slot,
    ) -> Result<Option<&'a str>, String> {
        if managed.providers.get(slot).is_some() {
            return Ok(managed
                .credentials
                .get(slot.credential_id())
                .map(String::as_str));
        }
        if matches!(slot, Slot::Default) {
            let legacy = self.effective(&managed.current)?;
            if !legacy.endpoint.is_empty() {
                return Ok(self.key(managed, &legacy));
            }
        }
        Ok(None)
    }
    pub(super) fn retained_slot_key<'a>(
        &'a self,
        managed: &'a ManagedModel,
        slot: Slot,
        fields: &ProviderFields,
    ) -> Option<&'a str> {
        if managed
            .providers
            .get(slot)
            .is_some_and(|current| current.same_service(fields))
            && let Some(key) = managed.credentials.get(slot.credential_id())
        {
            return Some(key);
        }
        managed
            .credentials
            .get(&slot.retained_credential_id(fields))
            .map(String::as_str)
    }
    pub(super) fn project_slot_credentials(
        &self,
        managed: &ManagedModel,
        projected: &mut BTreeMap<String, Value>,
    ) {
        for slot in [Slot::Default, Slot::Backup] {
            let current = managed.providers.get(slot);
            if let Some(fields) = current.filter(|fields| !fields.endpoint.is_empty()) {
                let id = slot.credential_id();
                projected.insert(id.into(), json!({"id":id,"endpoint":fields.endpoint,"wire_api":fields.wire_api,"api_key_present":managed.credentials.contains_key(id),"current":true,"deployment":false,"slot":slot.name()}));
            }
            for fields in managed.providers.history(slot) {
                if current.is_some_and(|current| current.same_service(fields))
                    && managed.credentials.contains_key(slot.credential_id())
                {
                    continue;
                }
                let id = slot.retained_credential_id(fields);
                projected.entry(id.clone()).or_insert_with(|| json!({"id":id,"endpoint":fields.endpoint,"wire_api":fields.wire_api,"api_key_present":self.retained_slot_key(managed,slot,fields).is_some(),"current":false,"deployment":false,"slot":slot.name()}));
            }
        }
    }
    pub(super) fn update_slot_credential(
        &self,
        managed: &mut ManagedModel,
        id: &str,
        value: Option<&str>,
    ) -> Result<bool, String> {
        for slot in [Slot::Default, Slot::Backup] {
            let current = managed.providers.get(slot);
            let fields = if id == slot.credential_id() {
                current
            } else {
                managed
                    .providers
                    .history(slot)
                    .find(|fields| slot.retained_credential_id(fields) == id)
            };
            let Some(fields) = fields.cloned() else {
                continue;
            };
            let current_service = current.is_some_and(|current| current.same_service(&fields));
            let retained_id = slot.retained_credential_id(&fields);
            if let Some(value) = value {
                fields.registry("validation-placeholder", value)?;
                let target = if current_service {
                    slot.credential_id().to_owned()
                } else {
                    retained_id.clone()
                };
                managed.credentials.insert(target, value.to_owned());
                if current_service {
                    managed.credentials.remove(&retained_id);
                }
            } else {
                managed.credentials.remove(&retained_id);
                if current_service {
                    managed.credentials.remove(slot.credential_id());
                }
            }
            return Ok(true);
        }
        Ok(false)
    }
    pub(super) fn provider_statuses(&self, managed: &ManagedModel) -> Result<Value, String> {
        let mut statuses = serde_json::Map::new();
        for (name, slot) in [("default", Slot::Default), ("backup", Slot::Backup)] {
            let (fields, source) = self.provider_fields(managed, slot)?;
            let key = self.provider_key(managed, slot)?;
            let configured = !fields.endpoint.is_empty()
                && !fields.flash.is_empty()
                && !fields.pro.is_empty()
                && key.is_some()
                && fields.validate(key).is_ok();
            statuses.insert(name.into(), json!({"fields":fields,"api_key_present":key.is_some(),"configured":configured,"source":source}));
        }
        Ok(statuses.into())
    }
    fn draft_provider(
        &self,
        managed: &mut ManagedModel,
        slot: Slot,
        update: ProviderUpdate,
    ) -> Result<ProviderFields, String> {
        let (before, _) = self.provider_fields(managed, slot)?;
        let saved_before = managed.providers.get(slot).cloned();
        let before_key = self.provider_key(managed, slot)?.map(str::to_owned);
        let mut fields = update.fields;
        for model in [&fields.flash, &fields.pro] {
            if !model.trim().is_empty() && model.chars().any(char::is_control) {
                return Err("model names must contain no control characters".into());
            }
        }
        fields.endpoint = fields.endpoint.trim().into();
        fields.flash = fields.flash.trim().into();
        fields.pro = fields.pro.trim().into();
        let clearing = matches!(update.api_key, KeyUpdate::Clear);
        let key = match update.api_key {
            KeyUpdate::Keep if before.same_service(&fields) => before_key.clone(),
            KeyUpdate::Set { value } => Some(value),
            KeyUpdate::Keep | KeyUpdate::Clear => None,
        };
        fields.validate(key.as_deref())?;
        if let Some(saved_before) = saved_before {
            if saved_before != fields {
                managed.providers.retain(slot, &saved_before);
            }
            if !saved_before.same_service(&fields)
                && let Some(before_key) = before_key
            {
                managed
                    .credentials
                    .insert(slot.retained_credential_id(&saved_before), before_key);
            }
        }
        if let Some(key) = key {
            managed.credentials.insert(slot.credential_id().into(), key);
            managed
                .credentials
                .remove(&slot.retained_credential_id(&fields));
        } else {
            managed.credentials.remove(slot.credential_id());
            if clearing {
                managed
                    .credentials
                    .remove(&slot.retained_credential_id(&fields));
            }
        }
        managed.providers.set(slot, fields.clone());
        Ok(fields)
    }
    pub(crate) fn save_provider(
        &self,
        slot: &str,
        update: ProviderUpdate,
    ) -> Result<Value, String> {
        let slot = Slot::parse(slot)?;
        let _lease = self.lease()?;
        let mut managed = self.load()?;
        self.draft_provider(&mut managed, slot, update)?;
        let status = self.project(&managed)?;
        self.publish(&managed)?;
        Ok(status)
    }
    pub(crate) async fn test_provider(
        &self,
        slot: &str,
        test: ProviderTest,
    ) -> Result<Value, String> {
        let slot = Slot::parse(slot)?;
        let mut managed = self.load()?;
        let fields = self.draft_provider(
            &mut managed,
            slot,
            ProviderUpdate {
                fields: test.fields,
                api_key: test.api_key,
            },
        )?;
        let model = match test.tier {
            Tier::Flash => &fields.flash,
            Tier::Pro => &fields.pro,
        };
        if model.is_empty() {
            return Err("the selected tier has no model configured".into());
        }
        let key = self
            .provider_key(&managed, slot)?
            .ok_or("the selected provider has no API key configured")?;
        let registry = fields.registry(model, key)?;
        connection_test::run(&registry, &fields.model_fields(model)).await
    }
}

#[cfg(test)]
mod tests;
