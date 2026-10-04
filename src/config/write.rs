#[derive(Clone, Copy)]
pub(crate) enum ConfigEdit<'a> {
    Theme(&'a str),
    StatusIndicators(super::StatusIndicatorStyle),
    WorkingSpinner(super::WorkingSpinnerStyle),
    SubAgentSpinner(super::SubAgentSpinnerStyle),
    Sound(bool),
    SidebarTree(bool),
    SidebarTreePanel(bool),
    ToastDelivery(super::ToastDelivery),
}

impl ConfigEdit<'_> {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Theme(_) => "theme",
            Self::StatusIndicators(_) => "status indicators",
            Self::WorkingSpinner(_) => "working spinner",
            Self::SubAgentSpinner(_) => "sub-agent spinner",
            Self::Sound(_) => "sound setting",
            Self::SidebarTree(_) => "sidebar tree setting",
            Self::SidebarTreePanel(_) => "sidebar tree panel setting",
            Self::ToastDelivery(_) => "toast setting",
        }
    }

    pub(crate) fn apply(self, content: &str) -> String {
        match self {
            Self::Theme(name) => {
                let content =
                    super::upsert_section_value(content, "theme", "name", &format!("\"{name}\""));
                super::upsert_section_bool(&content, "theme", "auto_switch", false)
            }
            Self::StatusIndicators(style) => super::upsert_section_value(
                content,
                "ui",
                "status_indicators",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::WorkingSpinner(style) => super::upsert_section_value(
                content,
                "ui",
                "working_spinner",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::SubAgentSpinner(style) => super::upsert_section_value(
                content,
                "ui",
                "sub_agent_spinner",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::Sound(enabled) => {
                super::upsert_section_bool(content, "ui.sound", "enabled", enabled)
            }
            Self::SidebarTree(enabled) => {
                super::upsert_section_bool(content, "ui", "sidebar_tree", enabled)
            }
            Self::SidebarTreePanel(enabled) => {
                super::upsert_section_bool(content, "ui", "sidebar_tree_panel", enabled)
            }
            Self::ToastDelivery(delivery) => {
                let value = match delivery {
                    super::ToastDelivery::Off => "\"off\"",
                    super::ToastDelivery::Herdr => "\"herdr\"",
                    super::ToastDelivery::Terminal => "\"terminal\"",
                    super::ToastDelivery::System => "\"system\"",
                };
                let content = super::upsert_section_value(content, "ui.toast", "delivery", value);
                super::remove_section_key(&content, "ui.toast", "enabled")
            }
        }
    }
}

pub(crate) fn update_file_at(
    path: &std::path::Path,
    description: &str,
    update: impl FnOnce(&str) -> String,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config directory: {error}"))?;
    }
    let content = match super::io::read_optional_config(path) {
        Ok(Some(content)) => content,
        Ok(None) => String::new(),
        Err(error) => {
            return Err(format!(
                "failed to read config before saving {description}: {error}"
            ));
        }
    };
    std::fs::write(path, update(&content))
        .map_err(|error| format!("failed to save {description}: {error}"))
}

pub(crate) fn write_edit(edit: ConfigEdit<'_>) -> Result<(), String> {
    update_file_at(&super::config_path(), edit.description(), |content| {
        edit.apply(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_tree_panel_edit_writes_its_ui_key() {
        let written = ConfigEdit::SidebarTreePanel(false).apply("[ui]\nsidebar_tree = true\n");
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert!(config.ui.sidebar_tree, "unexpected config {written:?}");
        assert!(
            !config.ui.sidebar_tree_panel,
            "unexpected config {written:?}"
        );

        let written = ConfigEdit::SidebarTreePanel(true).apply(&written);
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert!(
            config.ui.sidebar_tree_panel,
            "unexpected config {written:?}"
        );
        assert_eq!(written.matches("sidebar_tree_panel").count(), 1);
    }

    #[test]
    fn working_spinner_edit_writes_its_ui_key_and_keeps_the_others() {
        let written = ConfigEdit::WorkingSpinner(crate::config::WorkingSpinnerStyle::MoonPhases)
            .apply("[ui]\nstatus_indicators = \"symbols\"\n");
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(
            config.ui.working_spinner,
            crate::config::WorkingSpinnerStyle::MoonPhases,
            "unexpected config {written:?}"
        );
        assert_eq!(
            config.ui.status_indicators,
            crate::config::StatusIndicatorStyle::Symbols
        );
        let written =
            ConfigEdit::WorkingSpinner(crate::config::WorkingSpinnerStyle::Braille).apply(&written);
        assert!(
            written.contains("working_spinner = \"braille\""),
            "{written:?}"
        );
        assert_eq!(written.matches("working_spinner").count(), 1);
    }

    #[test]
    fn sub_agent_spinner_edit_writes_its_ui_key_and_keeps_the_main_one() {
        let written = ConfigEdit::SubAgentSpinner(crate::config::SubAgentSpinnerStyle::Arc)
            .apply("[ui]\nworking_spinner = \"moon_phases\"\n");
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(
            config.ui.sub_agent_spinner,
            crate::config::SubAgentSpinnerStyle::Arc,
            "unexpected config {written:?}"
        );
        assert_eq!(
            config.ui.working_spinner,
            crate::config::WorkingSpinnerStyle::MoonPhases
        );
        let written =
            ConfigEdit::SubAgentSpinner(crate::config::SubAgentSpinnerStyle::Auto).apply(&written);
        assert!(
            written.contains("sub_agent_spinner = \"auto\""),
            "{written:?}"
        );
        assert_eq!(written.matches("sub_agent_spinner").count(), 1);
    }

    #[test]
    fn sidebar_tree_edit_writes_ui_key_and_keeps_other_ui_settings() {
        let written = ConfigEdit::SidebarTree(true).apply("[ui]\nconfirm_close = false\n");
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert!(config.ui.sidebar_tree, "unexpected config {written:?}");
        assert!(!config.ui.confirm_close);

        let written = ConfigEdit::SidebarTree(false).apply(&written);
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert!(!config.ui.sidebar_tree, "unexpected config {written:?}");
        assert_eq!(written.matches("sidebar_tree").count(), 1);
    }

    #[test]
    fn update_file_at_does_not_move_a_leading_bom_into_the_file() {
        let dir = std::env::temp_dir().join(format!("herdr-config-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            b"\xEF\xBB\xBF[terminal]\ndefault_shell = \"pwsh.exe\"\n",
        )
        .unwrap();

        update_file_at(&path, "onboarding setting", |content| {
            crate::config::upsert_top_level_bool(content, "onboarding", false)
        })
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert!(
            !written.contains('\u{feff}'),
            "unexpected BOM in {written:?}"
        );
        assert!(
            toml::from_str::<toml::Value>(&written).is_ok(),
            "written config is not valid TOML: {written:?}"
        );
    }
}
