//! Application panel identities and persistence around the shared docking renderer.
//! Document contents remain owned by their existing view implementations.

use craft_ui::docking::{Action, DockArea, DockStyle, Layout, Location, Node, PanelLimits, Permissions, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Canvas,
    Library,
    Tools,
    Presets,
}

impl Panel {
    pub const ALL: &[Self] = &[Self::Canvas, Self::Library, Self::Tools, Self::Presets];
    pub fn id(self) -> &'static str {
        match self {
            Self::Canvas => "canvas",
            Self::Library => "library",
            Self::Tools => "tools",
            Self::Presets => "presets",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Photos",
            Self::Library => "Library",
            Self::Tools => "Tools",
            Self::Presets => "Presets",
        }
    }
    fn default_side(self) -> Zone {
        match self {
            Self::Canvas => Zone::Right,
            Self::Library => Zone::Left,
            Self::Tools => Zone::Right,
            Self::Presets => Zone::Right,
        }
    }
    fn parse(id: &str) -> Result<Self, String> {
        Self::ALL.iter().copied().find(|panel| panel.id() == id).ok_or_else(|| format!("Unknown panel: {id}"))
    }
}

fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Canvas { Permissions::PROTECTED } else { Permissions::default() }
}

/// The Presets panel's starting width (the side panels' are `state::LEFT_WIDTH` / `RIGHT_WIDTH`).
const PRESETS_WIDTH: f32 = 300.0;

/// The width a double-click on a panel's divider returns it to; the photo view has none.
fn default_width(panel: Panel) -> Option<f32> {
    match panel {
        Panel::Canvas => None,
        Panel::Library => Some(crate::state::LEFT_WIDTH.default),
        Panel::Tools => Some(crate::state::RIGHT_WIDTH.default),
        Panel::Presets => Some(PRESETS_WIDTH),
    }
}

fn tabs(panel: Panel) -> Node<Panel> {
    Node::Tabs { panels: vec![panel], active: 0 }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub layout: Layout<Panel>,
    /// Closed panels retain their most recent group, side, size or floating rectangle.
    pub hidden: Vec<(Panel, Location<Panel>)>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            layout: Layout {
                root: Some(split(
                    SplitAxis::Horizontal,
                    SplitSize::FixedFirst(crate::state::LEFT_WIDTH.default),
                    tabs(Panel::Library),
                    split(
                        SplitAxis::Horizontal,
                        SplitSize::FixedSecond(crate::state::RIGHT_WIDTH.default),
                        split(SplitAxis::Horizontal, SplitSize::FixedSecond(PRESETS_WIDTH), tabs(Panel::Canvas), tabs(Panel::Presets)),
                        tabs(Panel::Tools),
                    ),
                )),
                floating: Vec::new(),
            },
            hidden: Vec::new(),
        }
    }
}

impl Workspace {
    pub fn validate(&self) -> Result<(), String> {
        self.layout.validate().map_err(|error| error.to_string())?;
        if !self.layout.contains(&Panel::Canvas) || self.hidden.len() > Panel::ALL.len() {
            return Err("The panel layout must contain its document view and bounded hidden panels".into());
        }
        // Saved settings cannot turn the protected document view into a floating or closable tab.
        if self.layout.floating.iter().any(|group| group.panels.contains(&Panel::Canvas)) {
            return Err("The document view cannot float".into());
        }
        let mut nodes: Vec<_> = self.layout.root.iter().collect();
        while let Some(node) = nodes.pop() {
            match node {
                Node::Split { first, second, .. } => {
                    nodes.push(first);
                    nodes.push(second);
                }
                Node::Tabs { panels, .. } if panels.contains(&Panel::Canvas) && panels.len() != 1 => {
                    return Err("The document view cannot share a tab group".into());
                }
                Node::Stack { entries } if entries.iter().any(|entry| entry.panel == Panel::Canvas) => {
                    return Err("The document view cannot be collapsed".into());
                }
                _ => {}
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (panel, location) in &self.hidden {
            if *panel == Panel::Canvas || self.layout.contains(panel) || !seen.insert(*panel) {
                return Err("A hidden panel has an invalid or duplicate identity".into());
            }
            if location.anchor == Some(Panel::Canvas) && matches!(location.placement, Placement::Tab { .. } | Placement::Split(Zone::Center)) {
                return Err("A hidden panel cannot restore over the document view".into());
            }
            let mut probe = self.layout.clone();
            if let Err(error) = probe.restore(*panel, location)
                && error != craft_ui::docking::DockError::MissingTarget
            {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    pub fn apply(&mut self, action: Action<Panel>) -> Result<(), String> {
        self.validate()?;
        let mut next = self.clone();
        if let Action::Close { panel } = &action {
            let location = next.layout.location(panel).map_err(|error| error.to_string())?;
            next.hidden.retain(|(id, _)| id != panel);
            next.hidden.push((*panel, location));
        } else if let Action::Open { panel, .. } | Action::OpenAt { panel, .. } = &action {
            next.hidden.retain(|(id, _)| id != panel);
        }
        next.layout.apply_with_permissions(action, permissions).map_err(|error| error.to_string())?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn set_visible(&mut self, panel: Panel, visible: bool) -> Result<(), String> {
        if self.layout.contains(&panel) == visible {
            return Ok(());
        }
        if !visible {
            return self.apply(Action::Close { panel });
        }
        self.validate()?;
        let mut next = self.clone();
        let mut location = next.hidden.iter().find(|(id, _)| *id == panel).map(|(_, location)| location.clone());
        next.hidden.retain(|(id, _)| *id != panel);
        if let Some(saved) = &mut location
            && saved.floating.is_some()
            && saved.anchor.as_ref().is_some_and(|anchor| !next.layout.floating.iter().any(|group| group.panels.contains(anchor)))
        {
            // The other members of a floating group may have been closed too.
            saved.anchor = None;
            saved.placement = Placement::Tab { before: None };
        }
        let restored = location.as_ref().is_some_and(|location| next.layout.restore(panel, location).is_ok());
        if !restored {
            next.layout
                .apply_with_permissions(
                    Action::OpenAt { panel, anchor: Panel::Canvas, placement: Placement::Split(panel.default_side()) },
                    permissions,
                )
                .map_err(|error| error.to_string())?;
            // (put back beside the photo afresh: at the width it had, not half the room)
            let width = match location.as_ref().map(|l| (&l.placement, l.size)) {
                Some((Placement::Split(Zone::Left | Zone::Right), Some(SplitSize::FixedFirst(width) | SplitSize::FixedSecond(width)))) => Some(width),
                _ => default_width(panel),
            };
            if let Some(width) = width {
                next.set_width(panel, width)?;
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Membership is passive state; an explicit command also exposes the panel's body.
    pub fn reveal(&mut self, panel: Panel) -> Result<(), String> {
        self.set_visible(panel, true)?;
        self.apply(Action::Activate { panel })
    }

    pub fn exposed(&self, panel: Panel) -> bool {
        if self.layout.floating.iter().any(|group| group.panels.get(group.active) == Some(&panel)) {
            return true;
        }
        let mut nodes = self.layout.root.as_ref().map(|node| vec![node]).unwrap_or_default();
        while let Some(node) = nodes.pop() {
            match node {
                Node::Tabs { panels, active } if panels.get(*active) == Some(&panel) => return true,
                Node::Stack { entries } if entries.iter().any(|entry| entry.panel == panel && entry.open) => return true,
                Node::Split { first, second, .. } => {
                    nodes.push(first);
                    nodes.push(second);
                }
                _ => {}
            }
        }
        false
    }

    fn command(&mut self, params: &Value) -> Result<(), String> {
        self.validate()?;
        if let Some(action) = params.get("action") {
            let action: Action<Panel> = serde_json::from_value(action.clone()).map_err(|error| error.to_string())?;
            return self.apply(action);
        }
        let operation = params.get("operation").and_then(Value::as_str).ok_or("operation is required")?;
        if operation == "resizeSplit" {
            let path: Vec<bool> =
                serde_json::from_value(params.get("path").cloned().ok_or("path is required")?).map_err(|error| error.to_string())?;
            let size: SplitSize =
                serde_json::from_value(params.get("size").cloned().ok_or("size is required")?).map_err(|error| error.to_string())?;
            return self.apply(Action::ResizeSplit { path, size });
        }
        let panel = Panel::parse(params.get("panel").and_then(Value::as_str).ok_or("panel is required")?)?;
        let action = match operation {
            "open" => return self.reveal(panel),
            "close" => Action::Close { panel },
            "activate" => Action::Activate { panel },
            "resetWidth" => return self.set_width(panel, default_width(panel).ok_or("that panel has no width to reset")?),
            "setStackOpen" => Action::SetStackOpen { panel, open: params.get("open").and_then(Value::as_bool).ok_or("open must be a boolean")? },
            "resizeStack" => {
                let value = params.get("height").ok_or("height is required")?;
                let height = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_f64()
                            .filter(|height| height.is_finite() && (0.0..=1_000_000.0).contains(height))
                            .ok_or("height must be finite and bounded")? as f32,
                    )
                };
                Action::ResizeStack { panel, height }
            }
            "float" | "moveFloating" => {
                let rect = params.get("rect").and_then(Value::as_array).filter(|rect| rect.len() == 4).ok_or("rect must be [x,y,width,height]")?;
                let mut values = [0.0f32; 4];
                for (output, value) in values.iter_mut().zip(rect) {
                    *output = value
                        .as_f64()
                        .filter(|number| number.is_finite() && number.abs() <= 1_000_000.0)
                        .ok_or("rect values must be finite and bounded")? as f32;
                }
                if operation == "float" { Action::Float { panel, rect: values } } else { Action::MoveFloating { panel, rect: values } }
            }
            "move" => {
                let anchor = Panel::parse(params.get("target").and_then(Value::as_str).ok_or("target is required")?)?;
                let placement = match params.get("zone").and_then(Value::as_str).unwrap_or("center") {
                    "center" => Placement::Split(Zone::Center),
                    "left" => Placement::Split(Zone::Left),
                    "right" => Placement::Split(Zone::Right),
                    "top" => Placement::Split(Zone::Top),
                    "bottom" => Placement::Split(Zone::Bottom),
                    _ => return Err("zone must be center, left, right, top or bottom".into()),
                };
                let placement = if let Some(before) = params.get("before") {
                    if !matches!(placement, Placement::Split(Zone::Center)) {
                        return Err("before only applies to a center move".into());
                    }
                    Placement::Tab {
                        before: if before.is_null() {
                            None
                        } else {
                            Some(Panel::parse(before.as_str().ok_or("before must be a panel ID or null")?)?)
                        },
                    }
                } else {
                    placement
                };
                Action::Move { panel, anchor, placement }
            }
            _ => {
                return Err(
                    "operation must be open, close, activate, float, move, moveFloating, resizeSplit, resetWidth, setStackOpen or resizeStack".into(),
                );
            }
        };
        self.apply(action)
    }
}

fn from_ui(app: &crate::LightcraftApp) -> Workspace {
    let mut workspace = Workspace::default();
    if let Some(Node::Split { size, second, .. }) = workspace.layout.root.as_mut() {
        *size = SplitSize::FixedFirst(crate::state::LEFT_WIDTH.clamp(app.ui.left_width));
        if let Node::Split { size, .. } = second.as_mut() {
            *size = SplitSize::FixedSecond(crate::state::RIGHT_WIDTH.clamp(app.ui.right_width));
        }
    }
    workspace
}

fn sync(workspace: &mut Workspace, app: &crate::LightcraftApp) -> Result<(), String> {
    workspace.set_visible(Panel::Library, app.ui.left_panel)?;
    workspace.set_visible(Panel::Tools, app.ui.right != crate::state::RightPanel::None)?;
    workspace.set_visible(Panel::Presets, app.ui.presets)
}

fn publish_visibility(app: &mut crate::LightcraftApp, workspace: &Workspace) {
    app.ui.left_panel = workspace.layout.contains(&Panel::Library);
    app.ui.presets = workspace.layout.contains(&Panel::Presets);
    if !workspace.layout.contains(&Panel::Tools) {
        app.ui.right = crate::state::RightPanel::None;
    } else if app.ui.right == crate::state::RightPanel::None {
        app.ui.right = crate::state::RightPanel::Edit;
    }
    for (panel, width, limits) in
        [(Panel::Library, &mut app.ui.left_width, crate::state::LEFT_WIDTH), (Panel::Tools, &mut app.ui.right_width, crate::state::RIGHT_WIDTH)]
    {
        let Ok(location) = workspace.layout.location(&panel) else {
            continue;
        };
        let value = match (location.floating, location.placement, location.size) {
            (Some([_, _, width, _]), _, _)
            | (None, Placement::Split(Zone::Left), Some(SplitSize::FixedFirst(width)))
            | (None, Placement::Split(Zone::Right), Some(SplitSize::FixedSecond(width))) => Some(width),
            // A vertical split stores height, and the opposite fixed side sizes a sibling.
            _ => None,
        };
        if let Some(value) = value {
            *width = limits.clamp(value);
        }
    }
}

pub(crate) fn exposed(app: &crate::LightcraftApp, panel: Panel) -> bool {
    app.ui.docking.as_ref().is_none_or(|workspace| workspace.exposed(panel))
}

pub(crate) fn reveal_command(app: &mut crate::LightcraftApp, id: &str) -> Result<(), String> {
    let panel = if id == "panel.presets" && app.ui.presets {
        Some(Panel::Presets)
    } else if (id.starts_with("panel.") && id != "panel.close" && id != "panel.presets"
        || id.starts_with("section.")
        || id.starts_with("tool.") && id != "tool.none")
        && app.ui.right != crate::state::RightPanel::None
    {
        Some(Panel::Tools)
    } else {
        None
    };
    if let Some(panel) = panel {
        let mut workspace = app.ui.docking.clone().unwrap_or_else(|| from_ui(app));
        sync(&mut workspace, app)?;
        workspace.reveal(panel)?;
        cancel_replaced_gesture(app, Some(&workspace));
        app.ui.docking = Some(workspace);
    }
    Ok(())
}

/// Check the target's actual egui layer even while a different widget owns the drag.
pub(crate) fn drop_target(ui: &egui::Ui, rect: egui::Rect, pointer: Option<egui::Pos2>) -> bool {
    pointer.is_some_and(|pos| rect.intersect(ui.clip_rect()).contains(pos) && ui.ctx().layer_id_at(pos) == Some(ui.layer_id()))
}

/// Explicit host commands replace the snapshot before rendering. Cancel only their old
/// gestures; actions returned by the renderer must keep continuous resizing alive.
pub(crate) fn cancel_replaced_gesture(app: &crate::LightcraftApp, workspace: Option<&Workspace>) {
    let replaced = match (app.ui.docking.as_ref(), workspace) {
        (Some(old), Some(next)) => old.layout != next.layout || old.hidden != next.hidden,
        (None, None) => false,
        _ => true,
    };
    if replaced && let Some(ctx) = &app.tasks.repaint {
        craft_ui::docking::cancel_drag::<Panel>(ctx, egui::Id::new("lightcraft-docking"));
    }
}

pub fn command(app: &mut crate::LightcraftApp, reset: bool, params: &Value) -> Result<Value, String> {
    command_impl(app, reset, params, false)
}

fn command_impl(app: &mut crate::LightcraftApp, reset: bool, params: &Value, renderer_action: bool) -> Result<Value, String> {
    let mut workspace = app.ui.docking.clone().unwrap_or_else(|| from_ui(app));
    if reset {
        workspace = Workspace::default();
        // The usual LightCraft starting workspace shows its current tool beside the photo.
        workspace.set_visible(Panel::Library, false)?;
        workspace.set_visible(Panel::Presets, false)?;
    } else {
        sync(&mut workspace, app)?;
        workspace.command(params)?;
    }
    if !renderer_action {
        cancel_replaced_gesture(app, Some(&workspace));
    }
    publish_visibility(app, &workspace);
    app.ui.docking = Some(workspace);
    Ok(json!({"docking": app.ui.docking}))
}

pub fn show(app: &mut crate::LightcraftApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut workspace = app.ui.docking.clone().unwrap_or_else(|| from_ui(app));
    if let Err(error) = workspace.validate().and_then(|()| sync(&mut workspace, app)) {
        app.ui.toast = Some((format!("Panel layout could not be restored: {error}"), ui.input(|input| input.time) + 8.0, None));
        workspace = from_ui(app);
        let _ = sync(&mut workspace, app);
    }
    // Legacy visibility commands may replace the layout before this render starts.
    cancel_replaced_gesture(app, Some(&workspace));
    // Tool/Canvas callbacks dispatch app commands and must see the real workspace.
    app.ui.docking = Some(workspace.clone());
    let mut style = DockStyle::from_ui(ui);
    style.min_pane = 96.0;
    style.float_label = crate::i18n::tr("Float panel").into();
    style.close_label = crate::i18n::tr("Close panel").into();
    style.move_label = crate::i18n::tr("Move to group").into();
    style.panels_label = crate::i18n::tr("Panels").into();
    style.resize_label = crate::i18n::tr("Resize panels").into();
    style.resize_window_label = crate::i18n::tr("Resize panel window").into();
    style.background = t.chrome;
    style.tab_background = t.chrome;
    style.active_background = t.canvas;
    style.text = t.text;
    style.inactive_text = t.text_dim;
    style.border = egui::Stroke::new(1.0, t.divider);
    style.accent = t.accent;
    style.font = t.font(12.0);
    // The gaps between panes (the drag borders) show what is behind the dock. Left alone that is
    // eframe's near-black clear colour: invisible in the dark themes, harsh black bars in the light ones.
    let area = ui.available_rect_before_wrap();
    ui.painter().rect_filled(area, 0.0, t.divider);
    // Tool and preset panels publish hover previews before the photo consumes them.
    let mut canvas = None;
    // Where each panel's body is drawn, to tell which divider a double-click lands on.
    let mut bodies = Vec::new();
    // panels being shown or hidden slide in and out, the photo making room or taking it back
    let (drawn, slides) = sliding(ui.ctx(), &workspace);
    if !slides.is_empty() {
        style.min_pane = 0.0;
    }
    let output = DockArea::new(egui::Id::new("lightcraft-docking")).show_with_limits(
        ui,
        &drawn,
        &style,
        |panel| crate::i18n::tr(panel.label()).into(),
        permissions,
        |panel| {
            let mut limits = panel_limits(panel);
            if slides.iter().any(|s| s.panel == *panel) {
                limits.min.x = 0.0;
            }
            limits
        },
        |ui, panel| {
            let rect = ui.max_rect();
            bodies.push((*panel, rect));
            if *panel == Panel::Canvas {
                canvas = Some(ui.new_child(egui::UiBuilder::new().id_salt("photo-view").max_rect(rect)));
                return;
            }
            // (one sliding is drawn whole, the part not in yet past the edge it comes from)
            let whole = match slides.iter().find(|s| s.panel == *panel) {
                Some(s) if s.left => egui::Rect::from_min_max(egui::pos2(rect.right() - s.full, rect.top()), rect.max),
                Some(s) => egui::Rect::from_min_size(rect.min, egui::vec2(s.full, rect.height())),
                None => rect,
            };
            let ui = &mut ui.new_child(egui::UiBuilder::new().id_salt("panel-body").max_rect(whole).layout(*ui.layout()));
            match panel {
                Panel::Canvas => {}
                Panel::Library => {
                    crate::widgets::register(ui.ctx(), "panel:left_panel", rect);
                    crate::panels::left::show(app, ui);
                }
                Panel::Tools => {
                    crate::widgets::register(ui.ctx(), "panel:right_panel", rect);
                    // sliding away, it still shows the tool it had (the app has moved on); a new
                    // tool's controls fade in
                    let last = egui::Id::new("dock-tools-last");
                    let tool = match app.ui.right {
                        crate::state::RightPanel::None => ui.data(|d| d.get_temp(last)).unwrap_or(crate::state::RightPanel::None),
                        now => now,
                    };
                    ui.data_mut(|d| d.insert_temp(last, tool));
                    let had = std::mem::replace(&mut app.ui.right, tool);
                    ui.multiply_opacity(crate::panels::mobile::enter(ui.ctx(), egui::Id::new("dock-tools-in"), tool, crate::motion::FADE_S));
                    crate::panels::right::show(app, ui);
                    if app.ui.right == tool {
                        app.ui.right = had;
                    }
                }
                Panel::Presets => {
                    crate::widgets::register(ui.ctx(), "panel:presets", rect);
                    crate::panels::presets::show(app, ui);
                }
            }
        },
    );
    if let Some(mut canvas) = canvas {
        let ui = &mut canvas;
        crate::panels::bottombar::show(app, ui);
        let bg = if matches!(
            app.ui.view,
            crate::state::ViewMode::Detail | crate::state::ViewMode::Compare | crate::state::ViewMode::Survey | crate::state::ViewMode::Reference
        ) {
            t.canvas
        } else {
            t.grid_bg
        };
        crate::panels::compare::prepare(app);
        // a view fades in over the last (the photo grows out of its thumbnail, and flies back into it)
        crate::panels::detail::view_changed(app, ui.ctx());
        let grid = matches!(app.ui.view, crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid);
        let view = if grid { crate::state::ViewMode::PhotoGrid } else { app.ui.view };
        let k = crate::panels::mobile::enter(ui.ctx(), egui::Id::new("view-in"), view, crate::motion::FADE_S);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg)).show(ui, |ui| {
            ui.multiply_opacity(k);
            match app.ui.view {
                crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid => crate::panels::grid::show(app, ui),
                crate::state::ViewMode::Detail => crate::panels::detail::show(app, ui),
                crate::state::ViewMode::Compare => crate::panels::compare::show_compare(app, ui),
                crate::state::ViewMode::Survey => crate::panels::compare::show_survey(app, ui),
                crate::state::ViewMode::Reference => crate::panels::compare::show_reference(app, ui),
                crate::state::ViewMode::People => crate::panels::people::show(app, ui),
                crate::state::ViewMode::Map => crate::panels::map::show(app, ui),
            }
        });
    }

    let rendered = workspace;
    let mut workspace = app.ui.docking.clone().unwrap_or_else(|| from_ui(app));
    let _ = sync(&mut workspace, app);
    let stale = workspace.layout != rendered.layout || workspace.hidden != rendered.hidden;
    if stale {
        craft_ui::docking::cancel_drag::<Panel>(ui.ctx(), egui::Id::new("lightcraft-docking"));
    }
    publish_visibility(app, &workspace);
    app.ui.docking = Some(workspace);
    // (while panels slide, what was drawn isn't the layout: moves and resizes wait until they are in place)
    if stale || !slides.is_empty() {
        return;
    }
    for action in output.actions {
        if let Err(error) = serde_json::to_value(action)
            .map_err(|error| error.to_string())
            .and_then(|action| command_impl(app, false, &json!({"action": action}), true))
        {
            app.ui.toast = Some((format!("Panel move was not applied: {error}"), ui.input(|input| input.time) + 8.0, None));
        }
    }
    // A double-click on a divider returns the panel it sizes to its default width.
    let double_click =
        ui.input(|input| if input.pointer.button_double_clicked(egui::PointerButton::Primary) { input.pointer.interact_pos() } else { None });
    if let Some(pos) = double_click.filter(|pos| ui.ctx().layer_id_at(*pos) == Some(ui.layer_id())) {
        for panel in divider_owners(&rendered.layout, &bodies, style.gap, area, pos) {
            if let Err(error) = command_impl(app, false, &json!({"operation": "resetWidth", "panel": panel.id()}), true) {
                app.ui.toast = Some((format!("Panel width was not reset: {error}"), ui.input(|input| input.time) + 8.0, None));
            }
        }
    }
}

/// A side panel on its way in or out this frame: its width once it is all there, and whether it is
/// left of the photo (it slides in from the left edge) or right.
struct Slide {
    panel: Panel,
    full: f32,
    left: bool,
}

/// What to draw this frame: `workspace`'s layout with the panels being shown or hidden at the
/// width they have got to (one being hidden is put back where it was until it has gone). Only a
/// panel docked on its own beside the rest slides; one floating, tabbed or above or below another
/// comes and goes at once.
fn sliding(ctx: &egui::Context, workspace: &Workspace) -> (Layout<Panel>, Vec<Slide>) {
    let mut drawn = workspace.clone();
    let mut slides = Vec::new();
    for panel in [Panel::Library, Panel::Presets, Panel::Tools] {
        let open = workspace.layout.contains(&panel);
        let k = crate::motion::openness(ctx, egui::Id::new(("dock-slide", panel.id())), open, crate::motion::SLIDE_S);
        if (open && k >= 1.0) || (!open && k <= 0.0) {
            continue;
        }
        let mut next = drawn.clone();
        if !open && next.set_visible(panel, true).is_err() {
            continue;
        }
        if let Some((full, left)) = narrow(&mut next.layout, panel, k) {
            drawn = next;
            slides.push(Slide { panel, full, left });
        }
    }
    (drawn.layout, slides)
}

/// Make `panel`, docked on its own beside the rest, `k` of its width. Returns its whole width and
/// whether it is on the left; `None` (nothing changed) where it isn't docked so.
fn narrow(layout: &mut Layout<Panel>, panel: Panel, k: f32) -> Option<(f32, bool)> {
    let alone = |node: &Node<Panel>| matches!(node, Node::Tabs { panels, .. } if panels.as_slice() == [panel]);
    let mut nodes: Vec<&mut Node<Panel>> = layout.root.iter_mut().collect();
    while let Some(node) = nodes.pop() {
        let Node::Split { axis, size, first, second } = node else { continue };
        if *axis == SplitAxis::Horizontal {
            match *size {
                SplitSize::FixedFirst(width) if alone(first) => {
                    *size = SplitSize::FixedFirst(width * k);
                    return Some((width, true));
                }
                SplitSize::FixedSecond(width) if alone(second) => {
                    *size = SplitSize::FixedSecond(width * k);
                    return Some((width, false));
                }
                _ => {}
            }
        }
        nodes.push(first);
        nodes.push(second);
    }
    None
}

/// The panels whose width the divider at `pos` sets: the neighbour fixed on that side of its split
/// (the Presets | Tools divider sizes Tools, not Presets). `bodies` are where the panels were drawn.
fn divider_owners(layout: &Layout<Panel>, bodies: &[(Panel, egui::Rect)], gap: f32, area: egui::Rect, pos: egui::Pos2) -> Vec<Panel> {
    // craft-ui's divider handle also grabs this far to either side of the gap.
    const GRAB: f32 = 3.0;
    let owns = |panel: Panel, left_of_divider: bool| {
        layout.location(&panel).is_ok_and(|location| {
            location.floating.is_none()
                && if left_of_divider {
                    matches!((location.placement, location.size), (Placement::Split(Zone::Left), Some(SplitSize::FixedFirst(_))))
                } else {
                    matches!((location.placement, location.size), (Placement::Split(Zone::Right), Some(SplitSize::FixedSecond(_))))
                }
        })
    };
    let mut owners = Vec::new();
    if !area.contains(pos) {
        return owners;
    }
    for &(left, a) in bodies {
        for &(right, b) in bodies {
            let between = b.left() - a.right();
            if between > 0.0 && between <= gap + 0.5 && pos.x >= a.right() - GRAB && pos.x <= b.left() + GRAB {
                if owns(left, true) {
                    owners.push(left);
                }
                if owns(right, false) {
                    owners.push(right);
                }
            }
        }
    }
    owners
}

impl Workspace {
    /// Set `panel`'s width wherever it sits: closed (kept for when it reopens), floating, or beside a
    /// sibling (the width of its parent split; a vertical split's height is left alone).
    fn set_width(&mut self, panel: Panel, width: f32) -> Result<(), String> {
        if let Some((_, location)) = self.hidden.iter_mut().find(|(id, _)| *id == panel) {
            if let Some(rect) = &mut location.floating {
                rect[2] = width;
            }
            match location.placement {
                Placement::Split(Zone::Left) => location.size = Some(SplitSize::FixedFirst(width)),
                Placement::Split(Zone::Right) => location.size = Some(SplitSize::FixedSecond(width)),
                _ => {} // Width controls do not rewrite a saved vertical split's height.
            }
            return Ok(());
        }
        if let Some(group) = self.layout.floating.iter().find(|group| group.panels.contains(&panel)) {
            let mut rect = group.rect;
            rect[2] = width;
            return self.apply(Action::MoveFloating { panel, rect });
        }
        let mut nodes = self.layout.root.as_ref().map(|node| vec![(node, Vec::new())]).unwrap_or_default();
        let mut change = None;
        while let Some((node, path)) = nodes.pop() {
            if let Node::Split { axis, first, second, .. } = node {
                let contains = |node: &Node<Panel>| match node {
                    Node::Tabs { panels, .. } => panels.contains(&panel),
                    Node::Stack { entries } => entries.iter().any(|entry| entry.panel == panel),
                    Node::Split { .. } => false,
                };
                if *axis == SplitAxis::Horizontal {
                    if contains(first) {
                        change = Some(Action::ResizeSplit { path: path.clone(), size: SplitSize::FixedFirst(width) });
                    }
                    if contains(second) {
                        change = Some(Action::ResizeSplit { path: path.clone(), size: SplitSize::FixedSecond(width) });
                    }
                }
                let mut second_path = path.clone();
                second_path.push(true);
                let mut first_path = path;
                first_path.push(false);
                nodes.push((second, second_path));
                nodes.push((first, first_path));
            }
        }
        match change {
            Some(action) => self.apply(action),
            None => Ok(()),
        }
    }
}

/// Keep the established ui.set width controls connected to the panel's current parent split.
pub(crate) fn set_widths(state: &mut crate::UiState, params: &Value) -> Result<(), String> {
    state.left_width = crate::state::LEFT_WIDTH.clamp(state.left_width);
    state.right_width = crate::state::RIGHT_WIDTH.clamp(state.right_width);
    let Some(workspace) = state.docking.as_mut() else {
        return Ok(());
    };
    workspace.validate()?;
    for (key, panel, width) in [("leftWidth", Panel::Library, state.left_width), ("rightWidth", Panel::Tools, state.right_width)] {
        if params.get(key).is_some() {
            workspace.set_width(panel, width)?;
        }
    }
    Ok(())
}

fn panel_limits(panel: &Panel) -> PanelLimits {
    let (min, max_width) = match panel {
        Panel::Canvas => (egui::vec2(crate::state::MIN_PHOTO_WIDTH, 160.0), 1_000_000.0),
        Panel::Library => (egui::vec2(crate::state::LEFT_WIDTH.min, 120.0), crate::state::LEFT_WIDTH.max),
        Panel::Tools => (egui::vec2(crate::state::RIGHT_WIDTH.min, 120.0), crate::state::RIGHT_WIDTH.max),
        Panel::Presets => (egui::vec2(250.0, 120.0), 520.0),
    };
    PanelLimits { min, max: egui::vec2(max_width, 1_000_000.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_widths() -> crate::LightcraftApp {
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
        app.ui.left_width = 333.0;
        app.ui.right_width = 410.0;
        app
    }

    fn tools_split(axis: SplitAxis, size: SplitSize, first: bool) -> Workspace {
        let rest = split(
            SplitAxis::Horizontal,
            SplitSize::FixedFirst(333.0),
            tabs(Panel::Library),
            split(SplitAxis::Horizontal, SplitSize::FixedSecond(300.0), tabs(Panel::Canvas), tabs(Panel::Presets)),
        );
        let root = if first { split(axis, size, tabs(Panel::Tools), rest) } else { split(axis, size, rest, tabs(Panel::Tools)) };
        Workspace { layout: Layout { root: Some(root), floating: Vec::new() }, hidden: Vec::new() }
    }

    #[test]
    fn explicit_reveal_opens_collapsed_tools_and_survives_reload() {
        let mut app = app_with_widths();
        app.ui.right = crate::state::RightPanel::Crop;
        let entries = [Panel::Tools, Panel::Library, Panel::Presets]
            .into_iter()
            .map(|panel| craft_ui::docking::StackEntry { panel, open: panel != Panel::Tools, height: Some(180.0) })
            .collect();
        app.ui.docking = Some(Workspace {
            layout: Layout {
                root: Some(split(SplitAxis::Horizontal, SplitSize::FixedSecond(350.0), tabs(Panel::Canvas), Node::Stack { entries })),
                floating: Vec::new(),
            },
            hidden: Vec::new(),
        });
        let undo = app.session.undo.len();
        app.run("panel.crop", json!({})).unwrap();
        assert_eq!(app.ui.right, crate::state::RightPanel::Crop);
        let workspace = app.ui.docking.as_ref().unwrap();
        assert!(workspace.exposed(Panel::Tools));
        assert_eq!(app.session.undo.len(), undo);
        let saved = serde_json::to_string(workspace).unwrap();
        let reloaded: Workspace = serde_json::from_str(&saved).unwrap();
        reloaded.validate().unwrap();
        assert!(reloaded.exposed(Panel::Tools));
    }

    #[test]
    fn legacy_width_publication_ignores_heights_and_sibling_widths() {
        for (axis, size, first) in [
            (SplitAxis::Vertical, SplitSize::FixedFirst(180.0), true),
            (SplitAxis::Vertical, SplitSize::FixedSecond(180.0), false),
            (SplitAxis::Horizontal, SplitSize::FixedSecond(180.0), true),
            (SplitAxis::Horizontal, SplitSize::FixedFirst(180.0), false),
        ] {
            let workspace = tools_split(axis, size, first);
            workspace.validate().unwrap();
            let mut app = app_with_widths();
            publish_visibility(&mut app, &workspace);
            assert_eq!((app.ui.left_width, app.ui.right_width), (333.0, 410.0), "{axis:?} {size:?}, first={first}");
        }
    }

    #[test]
    fn legacy_width_publication_reads_own_horizontal_and_floating_widths() {
        for (size, first) in [(SplitSize::FixedFirst(310.0), true), (SplitSize::FixedSecond(310.0), false)] {
            let workspace = tools_split(SplitAxis::Horizontal, size, first);
            let mut app = app_with_widths();
            publish_visibility(&mut app, &workspace);
            assert_eq!((app.ui.left_width, app.ui.right_width), (333.0, 310.0));
        }
        let mut workspace = Workspace::default();
        workspace.apply(Action::Float { panel: Panel::Tools, rect: [12.0, 34.0, 390.0, 450.0] }).unwrap();
        let mut app = app_with_widths();
        publish_visibility(&mut app, &workspace);
        assert_eq!(app.ui.right_width, 390.0);
    }

    #[test]
    fn legacy_width_update_preserves_hidden_top_panel_height() {
        let mut workspace = Workspace::default();
        workspace.apply(Action::Move { panel: Panel::Tools, anchor: Panel::Canvas, placement: Placement::Split(Zone::Top) }).unwrap();
        workspace.set_visible(Panel::Tools, false).unwrap();
        let height = workspace.hidden.iter().find(|(panel, _)| *panel == Panel::Tools).unwrap().1.size;
        let mut state = crate::UiState { docking: Some(workspace), right_width: 410.0, ..Default::default() };
        set_widths(&mut state, &json!({"rightWidth":410.0})).unwrap();
        assert_eq!(state.docking.as_ref().unwrap().hidden.iter().find(|(panel, _)| *panel == Panel::Tools).unwrap().1.size, height);
    }
}
