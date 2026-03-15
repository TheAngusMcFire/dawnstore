use dawnstore_lib::{ResourceDefinition, ReturnObject};

/// Which view is currently rendered in the main area.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum View {
    /// Scrollable table of objects — the default landing view.
    #[default]
    ResourceList,
    /// Full-screen YAML detail for the selected object.
    Detail,
    /// Scrollable table of resource definitions.
    ResourceDefinitions,
    /// Full-screen detail for the selected resource definition (schema + aliases).
    ResourceDefinitionDetail,
    /// Vim-style `:` command bar overlaid on the resource list.
    CommandBar,
    /// Delete confirmation popup.
    Confirm,
    /// Namespace switcher popup.
    NsSwitcher,
    /// Keybinding help overlay.
    Help,
}

/// All application state. Pure data — no I/O. Mutated only by `update.rs`.
pub struct App {
    /// Currently active view.
    pub view: View,
    /// Active namespace (ignored when `all_namespaces` is true).
    pub namespace: String,
    /// Kind filter; `None` means show all kinds.
    pub kind_filter: Option<String>,
    /// When true the namespace filter is lifted and all objects are shown.
    pub all_namespaces: bool,
    /// Objects currently displayed in the resource list.
    pub objects: Vec<ReturnObject<serde_json::Value>>,
    /// Index of the highlighted row in the resource list.
    pub selected: usize,
    /// Whether the user is currently typing in the `/` name filter.
    pub filtering: bool,
    /// Live name filter string entered via `/`.
    pub name_filter: String,
    /// Namespaces available for the namespace switcher popup.
    pub namespaces: Vec<String>,
    /// Index of the highlighted row in the namespace switcher.
    pub ns_selected: usize,
    /// Current text in the `:` command bar input.
    pub command_input: String,
    /// Scroll offset in the detail YAML view (objects or resource definitions).
    pub detail_scroll: u16,
    /// Where to return when the confirm popup is dismissed.
    pub confirm_return_view: View,
    /// Success message shown in the footer.
    pub status: Option<String>,
    /// Error message shown in red in the footer.
    pub error: Option<String>,
    /// Ticks remaining before status/error is cleared (each tick ≈ 2 s).
    pub status_ticks: u8,
    /// While > 0, incoming ApiError events are silently dropped so that
    /// in-flight background refresh results don't override a user-dismissed error.
    /// Decremented on each tick; never affected by incoming errors.
    pub suppress_errors_ticks: u8,
    /// Resource definitions loaded by the `:rd` command.
    pub resource_definitions: Vec<ResourceDefinition>,
    /// Index of the highlighted row in the resource definitions list.
    pub rd_selected: usize,
    /// Basename of the context file shown in the header.
    pub context_name: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            view: View::default(),
            namespace: "default".to_string(),
            kind_filter: None,
            all_namespaces: false,
            objects: Vec::new(),
            selected: 0,
            filtering: false,
            name_filter: String::new(),
            namespaces: Vec::new(),
            ns_selected: 0,
            command_input: String::new(),
            detail_scroll: 0,
            confirm_return_view: View::ResourceList,
            status: None,
            error: None,
            status_ticks: 0,
            suppress_errors_ticks: 0,
            resource_definitions: Vec::new(),
            rd_selected: 0,
            context_name: String::new(),
        }
    }
}

impl App {
    /// Objects after applying the live name filter.
    pub fn visible_objects(&self) -> Vec<&ReturnObject<serde_json::Value>> {
        self.objects
            .iter()
            .filter(|o| {
                self.name_filter.is_empty() || o.name.contains(self.name_filter.as_str())
            })
            .collect()
    }

    /// The currently selected object, if any.
    pub fn selected_object(&self) -> Option<&ReturnObject<serde_json::Value>> {
        self.visible_objects().into_iter().nth(self.selected)
    }

    /// Clamp `selected` to valid range after objects change.
    pub fn clamp_selection(&mut self) {
        let max = self.visible_objects().len().saturating_sub(1);
        self.selected = self.selected.min(max);
    }
}
