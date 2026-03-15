use dawnstore_lib::ReturnObject;

/// Which view is currently rendered in the main area.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum View {
    /// Scrollable table of objects — the default landing view.
    #[default]
    ResourceList,
    /// Full-screen YAML detail for the selected object.
    Detail,
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
    /// Live name filter string entered via `/`.
    pub name_filter: String,
    /// Namespaces available for the namespace switcher popup.
    pub namespaces: Vec<String>,
    /// Index of the highlighted row in the namespace switcher.
    pub ns_selected: usize,
    /// Current text in the `:` command bar input.
    pub command_input: String,
    /// Scroll offset in the detail YAML view.
    pub detail_scroll: u16,
    /// Success message shown in the footer; cleared after a timeout tick.
    pub status: Option<String>,
    /// Error message shown in red in the footer; cleared after a timeout tick.
    pub error: Option<String>,
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
            name_filter: String::new(),
            namespaces: Vec::new(),
            ns_selected: 0,
            command_input: String::new(),
            detail_scroll: 0,
            status: None,
            error: None,
        }
    }
}

impl App {
    /// Objects after applying the live name filter.
    pub fn visible_objects(&self) -> Vec<&ReturnObject<serde_json::Value>> {
        self.objects
            .iter()
            .filter(|o| {
                self.name_filter.is_empty()
                    || o.name.contains(self.name_filter.as_str())
            })
            .collect()
    }

    /// The currently selected object, if any.
    pub fn selected_object(&self) -> Option<&ReturnObject<serde_json::Value>> {
        self.visible_objects().into_iter().nth(self.selected)
    }
}
