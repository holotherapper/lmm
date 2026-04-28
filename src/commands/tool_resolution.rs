//! Tool selection, validation, and interactive resolution helpers.
use crate::adapters::{
    AdapterClass, AdapterInfo, builtin_adapters, builtin_adapters_with_config, compatibility_reason,
};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::model::FormatKind;
use crate::tui;

use super::FormatSelection;

pub(crate) fn validate_tools(tools: &[String], format: FormatSelection) -> Result<()> {
    let adapters = builtin_adapters();
    for tool in tools {
        if tool != "all" && !adapters.iter().any(|adapter| adapter.id == tool) {
            return Err(AppError::UnknownAdapter(tool.to_string()));
        }
        if let Some(adapter) = adapters.iter().find(|a| a.id == tool)
            && adapter.class == AdapterClass::ImportingRegistry
        {
            return Err(AppError::InvalidInput(format!(
                "tool `{tool}` copies models to its own store, doubling disk usage. Use the tool's native CLI instead (e.g. `ollama pull`)."
            )));
        }
    }

    let FormatSelection::Format(format) = format else {
        return Ok(());
    };

    for tool in tools {
        if let Some(reason) = compatibility_reason(tool, &format) {
            return Err(AppError::UnsupportedToolFormat {
                tool: tool.to_string(),
                format: format.to_string(),
                reason,
            });
        }
    }

    Ok(())
}

pub(crate) fn tool_list(tools: &[String]) -> String {
    if tools.is_empty() {
        crate::format::dim("default")
    } else {
        tools.join(", ")
    }
}

fn is_actionable_adapter(adapter: &AdapterInfo) -> bool {
    matches!(
        adapter.class,
        AdapterClass::FilesystemExposure | AdapterClass::PathConsumer
    )
}

pub(crate) fn expand_tools(tools: &[String], format: FormatKind) -> Vec<String> {
    if tools.iter().any(|tool| tool == "all") {
        return builtin_adapters()
            .into_iter()
            .filter(|adapter| {
                adapter.can_expose
                    && adapter.formats.contains(&format)
                    && is_actionable_adapter(adapter)
            })
            .map(|adapter| adapter.id.to_string())
            .collect();
    }
    tools.to_vec()
}

pub(crate) fn resolve_tools(tools: &[String], format: FormatKind, config: &Config) -> Vec<String> {
    if !tools.is_empty() {
        return expand_tools(tools, format);
    }

    if !config.defaults.default_tools.is_empty() {
        return config.defaults.default_tools.clone();
    }

    builtin_adapters_with_config(config)
        .into_iter()
        .filter(|adapter| {
            adapter.can_expose
                && adapter.formats.contains(&format)
                && is_actionable_adapter(adapter)
                && adapter.configured
        })
        .map(|adapter| adapter.id.to_string())
        .collect()
}

pub(crate) fn auto_available_tools(format: FormatKind) -> Vec<String> {
    crate::adapters::hf_direct::DIRECT_HF_TOOLS
        .iter()
        .filter(|tool| tool.formats.contains(&format))
        .map(|tool| tool.display_name.to_string())
        .collect()
}

pub(crate) fn resolve_tools_interactive(
    tools: &[String],
    format: FormatKind,
    config: &Config,
    auto_confirm: bool,
) -> Result<Vec<String>> {
    if !tools.is_empty() || auto_confirm || !tui::can_run() {
        return Ok(resolve_tools(tools, format, config));
    }

    let default_tools = resolve_tools(tools, format, config);
    let actionable_classes = [AdapterClass::FilesystemExposure, AdapterClass::PathConsumer];
    let mut items = Vec::new();
    let mut initial = Vec::new();
    for adapter in builtin_adapters_with_config(config)
        .into_iter()
        .filter(|a| {
            a.can_expose
                && a.formats.contains(&format)
                && actionable_classes.contains(&a.class)
                && a.configured
        })
    {
        let preview = matches!(
            adapter.maturity,
            crate::adapters::AdapterMaturity::Experimental
        );
        let label = if preview {
            format!("{} (preview) ({})", adapter.display_name, adapter.id)
        } else {
            format!("{} ({})", adapter.display_name, adapter.id)
        };
        let id = adapter.id.to_string();
        if !preview && default_tools.iter().any(|tool| tool == adapter.id) {
            initial.push(id.clone());
        }
        items.push((id, label, adapter_detail(adapter.class)));
    }
    tui::select_many("Expose to tools", &items, &initial)
}

pub(crate) fn adapter_detail(class: AdapterClass) -> String {
    match class {
        AdapterClass::FilesystemExposure => "symlink exposure".to_string(),
        AdapterClass::DirectHfReader => "reads HF cache directly".to_string(),
        AdapterClass::PathConsumer => "uses canonical file path".to_string(),
        AdapterClass::ImportingRegistry => "imports through tool registry".to_string(),
    }
}
