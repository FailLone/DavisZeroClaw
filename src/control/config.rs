use crate::{ControlConfig, RuntimePaths};
use anyhow::{anyhow, Result};
use std::fs;

pub fn load_control_config(paths: &RuntimePaths) -> Result<ControlConfig> {
    let path = paths.control_aliases_path();
    let raw = fs::read_to_string(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("control config not found: {}", path.display())
        } else {
            anyhow!("failed to read control config {}: {err}", path.display())
        }
    })?;
    let loaded = serde_json::from_str::<ControlConfig>(&raw)
        .map_err(|err| anyhow!("invalid control config {}: {err}", path.display()))?;
    let mut config = ControlConfig::default();
    merge_control_config(&mut config, loaded);
    Ok(config)
}

fn merge_control_config(base: &mut ControlConfig, override_cfg: ControlConfig) {
    base.entity_aliases.extend(override_cfg.entity_aliases);
    base.area_aliases.extend(override_cfg.area_aliases);
    base.groups.extend(override_cfg.groups);
    base.domain_preferences
        .extend(override_cfg.domain_preferences);
    if !override_cfg.room_tokens.is_empty() {
        base.room_tokens = override_cfg.room_tokens;
    }
    if !override_cfg.ignored_entities.is_empty() {
        base.ignored_entities = override_cfg.ignored_entities;
    }
}
