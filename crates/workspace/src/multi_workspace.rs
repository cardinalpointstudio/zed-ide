use anyhow::Result;
use feature_flags::{AgentV2FeatureFlag, FeatureFlagAppExt};
use gpui::{
    AnyView, App, Context, DragMoveEvent, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    ManagedView, MouseButton, Pixels, Render, SharedString, Subscription, Task, Tiling, Window,
    actions, deferred, px,
};
use project::Project;
use std::path::PathBuf;
use ui::{
    ButtonSize, Color, IconButton, IconButtonShape, IconName, IconSize, Tab, TabBar, TabCloseSide,
    TabPosition, Tooltip, prelude::*,
};

const SIDEBAR_RESIZE_HANDLE_SIZE: Pixels = px(6.0);

use crate::{
    DockPosition, Item, ModalView, Panel, Workspace, WorkspaceId, client_side_decorations,
};

actions!(
    multi_workspace,
    [
        /// Creates a new workspace within the current window.
        NewWorkspaceInWindow,
        /// Switches to the next workspace within the current window.
        NextWorkspaceInWindow,
        /// Switches to the previous workspace within the current window.
        PreviousWorkspaceInWindow,
        /// Toggles the workspace switcher sidebar.
        ToggleWorkspaceSidebar,
        /// Moves focus to or from the workspace sidebar without closing it.
        FocusWorkspaceSidebar,
    ]
);

pub enum SidebarEvent {
    Open,
    Close,
}

pub trait Sidebar: EventEmitter<SidebarEvent> + Focusable + Render + Sized {
    fn width(&self, cx: &App) -> Pixels;
    fn set_width(&mut self, width: Option<Pixels>, cx: &mut Context<Self>);
    fn has_notifications(&self, cx: &App) -> bool;
}

pub trait SidebarHandle: 'static + Send + Sync {
    fn width(&self, cx: &App) -> Pixels;
    fn set_width(&self, width: Option<Pixels>, cx: &mut App);
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn focus(&self, window: &mut Window, cx: &mut App);
    fn has_notifications(&self, cx: &App) -> bool;
    fn to_any(&self) -> AnyView;
    fn entity_id(&self) -> EntityId;
}

#[derive(Clone)]
pub struct DraggedSidebar;

impl Render for DraggedSidebar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

#[derive(Clone)]
pub struct DraggedWorkspaceTab {
    #[allow(dead_code)]
    pub workspace: Entity<Workspace>,
    pub index: usize,
    pub name: SharedString,
}

impl Render for DraggedWorkspaceTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Tab::new("dragged_workspace_tab")
            .toggle_state(true)
            .child(self.name.clone())
    }
}

impl<T: Sidebar> SidebarHandle for Entity<T> {
    fn width(&self, cx: &App) -> Pixels {
        self.read(cx).width(cx)
    }

    fn set_width(&self, width: Option<Pixels>, cx: &mut App) {
        self.update(cx, |this, cx| this.set_width(width, cx))
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn has_notifications(&self, cx: &App) -> bool {
        self.read(cx).has_notifications(cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn entity_id(&self) -> EntityId {
        Entity::entity_id(self)
    }
}

pub struct MultiWorkspace {
    workspaces: Vec<Entity<Workspace>>,
    active_workspace_index: usize,
    sidebar: Option<Box<dyn SidebarHandle>>,
    sidebar_open: bool,
    _sidebar_subscription: Option<Subscription>,
}

impl MultiWorkspace {
    pub fn new(workspace: Entity<Workspace>, _cx: &mut Context<Self>) -> Self {
        Self {
            workspaces: vec![workspace],
            active_workspace_index: 0,
            sidebar: None,
            sidebar_open: false,
            _sidebar_subscription: None,
        }
    }

    pub fn register_sidebar<T: Sidebar>(
        &mut self,
        sidebar: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription =
            cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| match event {
                SidebarEvent::Open => this.toggle_sidebar(window, cx),
                SidebarEvent::Close => {
                    this.close_sidebar(window, cx);
                }
            });
        self.sidebar = Some(Box::new(sidebar));
        self._sidebar_subscription = Some(subscription);
    }

    pub fn sidebar(&self) -> Option<&dyn SidebarHandle> {
        self.sidebar.as_deref()
    }

    pub fn sidebar_open(&self) -> bool {
        self.sidebar_open && self.sidebar.is_some()
    }

    pub fn sidebar_has_notifications(&self, cx: &App) -> bool {
        self.sidebar
            .as_ref()
            .map_or(false, |s| s.has_notifications(cx))
    }

    pub(crate) fn multi_workspace_enabled(&self, cx: &App) -> bool {
        cx.has_flag::<AgentV2FeatureFlag>()
    }

    pub fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_workspace_enabled(cx) {
            return;
        }

        if self.sidebar_open {
            self.close_sidebar(window, cx);
        } else {
            self.open_sidebar(window, cx);
            if let Some(sidebar) = &self.sidebar {
                sidebar.focus(window, cx);
            }
        }
    }

    pub fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_workspace_enabled(cx) {
            return;
        }

        if self.sidebar_open {
            let sidebar_is_focused = self
                .sidebar
                .as_ref()
                .is_some_and(|s| s.focus_handle(cx).contains_focused(window, cx));

            if sidebar_is_focused {
                let pane = self.workspace().read(cx).active_pane().clone();
                let pane_focus = pane.read(cx).focus_handle(cx);
                window.focus(&pane_focus, cx);
            } else if let Some(sidebar) = &self.sidebar {
                sidebar.focus(window, cx);
            }
        } else {
            self.open_sidebar(window, cx);
            if let Some(sidebar) = &self.sidebar {
                sidebar.focus(window, cx);
            }
        }
    }

    pub fn open_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = true;
        for workspace in &self.workspaces {
            workspace.update(cx, |workspace, cx| {
                workspace.set_workspace_sidebar_open(true, cx);
            });
        }
        self.serialize(window, cx);
        cx.notify();
    }

    fn close_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = false;
        for workspace in &self.workspaces {
            workspace.update(cx, |workspace, cx| {
                workspace.set_workspace_sidebar_open(false, cx);
            });
        }
        let pane = self.workspace().read(cx).active_pane().clone();
        let pane_focus = pane.read(cx).focus_handle(cx);
        window.focus(&pane_focus, cx);
        self.serialize(window, cx);
        cx.notify();
    }

    pub fn is_sidebar_open(&self) -> bool {
        self.sidebar_open
    }

    pub fn workspace(&self) -> &Entity<Workspace> {
        &self.workspaces[self.active_workspace_index]
    }

    pub fn workspaces(&self) -> &[Entity<Workspace>] {
        &self.workspaces
    }

    pub fn active_workspace_index(&self) -> usize {
        self.active_workspace_index
    }

    pub fn activate(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) {
        if !self.multi_workspace_enabled(cx) {
            self.workspaces[0] = workspace;
            self.active_workspace_index = 0;
            cx.notify();
            return;
        }

        let index = self.add_workspace(workspace, cx);
        if self.active_workspace_index != index {
            self.active_workspace_index = index;
            cx.notify();
        }
    }

    /// Adds a workspace to this window without changing which workspace is active.
    /// Returns the index of the workspace (existing or newly inserted).
    pub fn add_workspace(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> usize {
        if let Some(index) = self.workspaces.iter().position(|w| *w == workspace) {
            index
        } else {
            if self.sidebar_open {
                workspace.update(cx, |workspace, cx| {
                    workspace.set_workspace_sidebar_open(true, cx);
                });
            }
            self.workspaces.push(workspace);
            cx.notify();
            self.workspaces.len() - 1
        }
    }

    pub fn activate_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(
            index < self.workspaces.len(),
            "workspace index out of bounds"
        );
        self.active_workspace_index = index;
        self.serialize(window, cx);
        self.focus_active_workspace(window, cx);
        cx.notify();
    }

    pub fn activate_next_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.len() > 1 {
            let next_index = (self.active_workspace_index + 1) % self.workspaces.len();
            self.activate_index(next_index, window, cx);
        }
    }

    pub fn activate_previous_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.len() > 1 {
            let prev_index = if self.active_workspace_index == 0 {
                self.workspaces.len() - 1
            } else {
                self.active_workspace_index - 1
            };
            self.activate_index(prev_index, window, cx);
        }
    }

    fn serialize(&self, window: &mut Window, cx: &mut App) {
        let window_id = window.window_handle().window_id();
        let state = crate::persistence::model::MultiWorkspaceState {
            active_workspace_id: self.workspace().read(cx).database_id(),
            sidebar_open: self.sidebar_open,
        };
        cx.background_spawn(async move {
            crate::persistence::write_multi_workspace_state(window_id, state).await;
        })
        .detach();
    }

    fn focus_active_workspace(&self, window: &mut Window, cx: &mut App) {
        // If a dock panel is zoomed, focus it instead of the center pane.
        // Otherwise, focusing the center pane triggers dismiss_zoomed_items_to_reveal
        // which closes the zoomed dock.
        let focus_handle = {
            let workspace = self.workspace().read(cx);
            let mut target = None;
            for dock in workspace.all_docks() {
                let dock = dock.read(cx);
                if dock.is_open() {
                    if let Some(panel) = dock.active_panel() {
                        if panel.is_zoomed(window, cx) {
                            target = Some(panel.panel_focus_handle(cx));
                            break;
                        }
                    }
                }
            }
            target.unwrap_or_else(|| {
                let pane = workspace.active_pane().clone();
                pane.read(cx).focus_handle(cx)
            })
        };
        window.focus(&focus_handle, cx);
    }

    pub fn panel<T: Panel>(&self, cx: &App) -> Option<Entity<T>> {
        self.workspace().read(cx).panel::<T>(cx)
    }

    pub fn active_modal<V: ManagedView + 'static>(&self, cx: &App) -> Option<Entity<V>> {
        self.workspace().read(cx).active_modal::<V>(cx)
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.add_panel(panel, window, cx);
        });
    }

    pub fn focus_panel<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<T>> {
        self.workspace()
            .update(cx, |workspace, cx| workspace.focus_panel::<T>(window, cx))
    }

    pub fn toggle_modal<V: ModalView, B>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: B,
    ) where
        B: FnOnce(&mut Window, &mut gpui::Context<V>) -> V,
    {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, build);
        });
    }

    pub fn toggle_dock(
        &mut self,
        dock_side: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_dock(dock_side, window, cx);
        });
    }

    pub fn active_item_as<I: 'static>(&self, cx: &App) -> Option<Entity<I>> {
        self.workspace().read(cx).active_item_as::<I>(cx)
    }

    pub fn items_of_type<'a, T: Item>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + Iterator<Item = Entity<T>> {
        self.workspace().read(cx).items_of_type::<T>(cx)
    }

    pub fn database_id(&self, cx: &App) -> Option<WorkspaceId> {
        self.workspace().read(cx).database_id()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_random_database_id(&mut self, cx: &mut Context<Self>) {
        self.workspace().update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        Self::new(workspace, cx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_add_workspace(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Workspace> {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        self.activate(workspace.clone(), cx);
        workspace
    }

    pub fn create_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_workspace_enabled(cx) {
            return;
        }
        let app_state = self.workspace().read(cx).app_state().clone();
        let project = Project::local(
            app_state.client.clone(),
            app_state.node_runtime.clone(),
            app_state.user_store.clone(),
            app_state.languages.clone(),
            app_state.fs.clone(),
            None,
            project::LocalProjectFlags::default(),
            cx,
        );
        let new_workspace = cx.new(|cx| Workspace::new(None, project, app_state, window, cx));
        self.activate(new_workspace, cx);
        self.focus_active_workspace(window, cx);
    }

    pub fn remove_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.len() <= 1 || index >= self.workspaces.len() {
            return;
        }

        self.workspaces.remove(index);

        if self.active_workspace_index >= self.workspaces.len() {
            self.active_workspace_index = self.workspaces.len() - 1;
        } else if self.active_workspace_index > index {
            self.active_workspace_index -= 1;
        }

        self.focus_active_workspace(window, cx);
        cx.notify();
    }

    /// Moves a workspace from one index to another, updating active_workspace_index accordingly.
    pub fn move_workspace(
        &mut self,
        from_index: usize,
        to_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_index == to_index
            || from_index >= self.workspaces.len()
            || to_index >= self.workspaces.len()
        {
            return;
        }

        let workspace = self.workspaces.remove(from_index);
        self.workspaces.insert(to_index, workspace);

        // Update active_workspace_index to track the moved workspace if it was active
        if self.active_workspace_index == from_index {
            self.active_workspace_index = to_index;
        } else if from_index < self.active_workspace_index
            && to_index >= self.active_workspace_index
        {
            self.active_workspace_index -= 1;
        } else if from_index > self.active_workspace_index
            && to_index <= self.active_workspace_index
        {
            self.active_workspace_index += 1;
        }

        self.serialize(window, cx);
        cx.notify();
    }

    pub fn open_project(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let workspace = self.workspace().clone();

        if self.multi_workspace_enabled(cx) {
            workspace.update(cx, |workspace, cx| {
                workspace.open_workspace_for_paths(true, paths, window, cx)
            })
        } else {
            cx.spawn_in(window, async move |_this, cx| {
                let should_continue = workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.prepare_to_close(crate::CloseIntent::ReplaceWindow, window, cx)
                    })?
                    .await?;
                if should_continue {
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            workspace.open_workspace_for_paths(true, paths, window, cx)
                        })?
                        .await
                } else {
                    Ok(())
                }
            })
        }
    }

    fn render_workspace_tab(
        &self,
        workspace: &Entity<Workspace>,
        index: usize,
        total_count: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = index == self.active_workspace_index;
        let workspace_name = workspace.read(cx).display_name(cx);
        let workspace_clone = workspace.clone();
        let weak = cx.weak_entity();

        let position = if index == 0 {
            TabPosition::First
        } else if index == total_count - 1 {
            TabPosition::Last
        } else {
            TabPosition::Middle(index.cmp(&self.active_workspace_index))
        };

        let tab_name = workspace_name.clone();
        Tab::new(("workspace_tab", index))
            .position(position)
            .toggle_state(is_active)
            .close_side(TabCloseSide::End)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_index(index, window, cx);
            }))
            .on_drag(
                DraggedWorkspaceTab {
                    workspace: workspace_clone,
                    index,
                    name: tab_name,
                },
                |tab, _, _, cx| cx.new(|_| tab.clone()),
            )
            .drag_over::<DraggedWorkspaceTab>(move |tab, dragged, _, cx| {
                let styled = tab
                    .bg(cx.theme().colors().drop_target_background)
                    .border_color(cx.theme().colors().drop_target_border);

                if index < dragged.index {
                    styled.border_l_2()
                } else if index > dragged.index {
                    styled.border_r_2()
                } else {
                    styled
                }
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedWorkspaceTab, window, cx| {
                    this.move_workspace(dragged.index, index, window, cx);
                }),
            )
            .child(workspace_name)
            .end_slot::<AnyElement>(if total_count > 1 {
                Some(
                    IconButton::new(("close_workspace", index), IconName::Close)
                        .shape(IconButtonShape::Square)
                        .icon_color(Color::Muted)
                        .size(ButtonSize::None)
                        .icon_size(IconSize::XSmall)
                        .visible_on_hover("")
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| {
                                this.remove_workspace(index, window, cx);
                            })
                            .ok();
                        })
                        .tooltip(Tooltip::text("Close Project"))
                        .into_any_element(),
                )
            } else {
                None
            })
    }

    fn render_workspace_tab_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace_count = self.workspaces.len();
        let weak = cx.weak_entity();

        TabBar::new("workspace_tab_bar")
            .children(
                self.workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, workspace)| {
                        self.render_workspace_tab(workspace, index, workspace_count, window, cx)
                            .into_any_element()
                    }),
            )
            .end_child(
                IconButton::new("add_workspace", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            this.create_workspace(window, cx);
                        })
                        .ok();
                    })
                    .tooltip(Tooltip::text("New Project")),
            )
    }
}

impl Render for MultiWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let multi_workspace_enabled = self.multi_workspace_enabled(cx);
        let show_workspace_tabs = multi_workspace_enabled && self.workspaces.len() > 1;

        let sidebar: Option<AnyElement> = if multi_workspace_enabled && self.sidebar_open {
            self.sidebar.as_ref().map(|sidebar_handle| {
                let weak = cx.weak_entity();

                let sidebar_width = sidebar_handle.width(cx);
                let resize_handle = deferred(
                    div()
                        .id("sidebar-resize-handle")
                        .absolute()
                        .right(-SIDEBAR_RESIZE_HANDLE_SIZE / 2.)
                        .top(px(0.))
                        .h_full()
                        .w(SIDEBAR_RESIZE_HANDLE_SIZE)
                        .cursor_col_resize()
                        .on_drag(DraggedSidebar, |dragged, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| dragged.clone())
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_up(MouseButton::Left, move |event, _, cx| {
                            if event.click_count == 2 {
                                weak.update(cx, |this, cx| {
                                    if let Some(sidebar) = this.sidebar.as_mut() {
                                        sidebar.set_width(None, cx);
                                    }
                                })
                                .ok();
                                cx.stop_propagation();
                            }
                        })
                        .occlude(),
                );

                div()
                    .id("sidebar-container")
                    .relative()
                    .h_full()
                    .w(sidebar_width)
                    .flex_shrink_0()
                    .child(sidebar_handle.to_any())
                    .child(resize_handle)
                    .into_any_element()
            })
        } else {
            None
        };

        let workspace_tab_bar: Option<AnyElement> = if show_workspace_tabs {
            Some(self.render_workspace_tab_bar(window, cx).into_any_element())
        } else {
            None
        };

        client_side_decorations(
            v_flex()
                .key_context("Workspace")
                .size_full()
                .on_action(
                    cx.listener(|this: &mut Self, _: &NewWorkspaceInWindow, window, cx| {
                        this.create_workspace(window, cx);
                    }),
                )
                .on_action(
                    cx.listener(|this: &mut Self, _: &NextWorkspaceInWindow, window, cx| {
                        this.activate_next_workspace(window, cx);
                    }),
                )
                .on_action(cx.listener(
                    |this: &mut Self, _: &PreviousWorkspaceInWindow, window, cx| {
                        this.activate_previous_workspace(window, cx);
                    },
                ))
                .on_action(cx.listener(
                    |this: &mut Self, _: &ToggleWorkspaceSidebar, window, cx| {
                        this.toggle_sidebar(window, cx);
                    },
                ))
                .on_action(
                    cx.listener(|this: &mut Self, _: &FocusWorkspaceSidebar, window, cx| {
                        this.focus_sidebar(window, cx);
                    }),
                )
                .children(workspace_tab_bar)
                .child(
                    h_flex()
                        .flex_1()
                        .size_full()
                        .overflow_hidden()
                        .when(self.sidebar_open() && multi_workspace_enabled, |this| {
                            this.on_drag_move(cx.listener(
                                |this: &mut Self,
                                 e: &DragMoveEvent<DraggedSidebar>,
                                 _window,
                                 cx| {
                                    if let Some(sidebar) = &this.sidebar {
                                        let new_width = e.event.position.x;
                                        sidebar.set_width(Some(new_width), cx);
                                    }
                                },
                            ))
                            .children(sidebar)
                        })
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .size_full()
                                .overflow_hidden()
                                .child(self.workspace().clone()),
                        ),
                ),
            window,
            cx,
            Tiling {
                left: multi_workspace_enabled && self.sidebar_open,
                ..Tiling::default()
            },
        )
    }
}
