use crate::{
    project::{self, Project},
    theme, worktree, Settings,
};
use gpui::{
    action,
    elements::{Label, MouseEventHandler, UniformList, UniformListState},
    keymap::{
        self,
        menu::{SelectNext, SelectPrev},
        Binding,
    },
    platform::CursorStyle,
    AppContext, Element, ElementBox, Entity, ModelHandle, MutableAppContext, ReadModel, View,
    ViewContext, WeakViewHandle,
};
use postage::watch;
use std::{
    collections::{hash_map, HashMap},
    ffi::OsStr,
    ops::Range,
};

pub struct ProjectPanel {
    project: ModelHandle<Project>,
    list: UniformListState,
    visible_entries: Vec<Vec<usize>>,
    expanded_dir_ids: HashMap<usize, Vec<usize>>,
    active_entry: Option<ActiveEntry>,
    settings: watch::Receiver<Settings>,
    handle: WeakViewHandle<Self>,
}

#[derive(Copy, Clone)]
struct ActiveEntry {
    worktree_id: usize,
    entry_id: usize,
    index: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct EntryDetails {
    filename: String,
    depth: usize,
    is_dir: bool,
    is_expanded: bool,
    is_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectEntry {
    pub worktree_ix: usize,
    pub entry_id: usize,
}

action!(ExpandActiveEntry);
action!(CollapseActiveEntry);
action!(ToggleExpanded, ProjectEntry);
action!(Open, ProjectEntry);

pub fn init(cx: &mut MutableAppContext) {
    cx.add_action(ProjectPanel::expand_active_entry);
    cx.add_action(ProjectPanel::collapse_active_entry);
    cx.add_action(ProjectPanel::toggle_expanded);
    cx.add_action(ProjectPanel::select_prev);
    cx.add_action(ProjectPanel::select_next);
    cx.add_bindings([
        Binding::new("right", ExpandActiveEntry, None),
        Binding::new("left", CollapseActiveEntry, None),
    ]);
}

pub enum Event {}

impl ProjectPanel {
    pub fn new(
        project: ModelHandle<Project>,
        settings: watch::Receiver<Settings>,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        cx.observe(&project, |this, _, cx| {
            this.update_visible_entries(None, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe(&project, |this, _, event, cx| match event {
            project::Event::ActiveEntryChanged(entry) => {
                if let Some((worktree_id, entry_id)) = *entry {
                    this.expand_entry(worktree_id, entry_id, cx);
                    this.update_visible_entries(Some((worktree_id, entry_id)), cx);
                    cx.notify();
                }
            }
            project::Event::WorktreeRemoved(id) => {
                this.expanded_dir_ids.remove(id);
                this.update_visible_entries(None, cx);
                cx.notify();
            }
        })
        .detach();

        let mut this = Self {
            project,
            settings,
            list: Default::default(),
            visible_entries: Default::default(),
            expanded_dir_ids: Default::default(),
            active_entry: None,
            handle: cx.handle().downgrade(),
        };
        this.update_visible_entries(None, cx);
        this
    }

    fn expand_active_entry(&mut self, _: &ExpandActiveEntry, cx: &mut ViewContext<Self>) {
        if let Some(active_entry) = self.active_entry {
            let project = self.project.read(cx);
            if let Some(worktree) = project.worktree_for_id(active_entry.worktree_id) {
                if let Some(entry) = worktree.read(cx).entry_for_id(active_entry.entry_id) {
                    if entry.is_dir() {
                        if let Some(expanded_dir_ids) =
                            self.expanded_dir_ids.get_mut(&active_entry.worktree_id)
                        {
                            match expanded_dir_ids.binary_search(&active_entry.entry_id) {
                                Ok(_) => self.select_next(&SelectNext, cx),
                                Err(ix) => {
                                    expanded_dir_ids.insert(ix, active_entry.entry_id);
                                    self.update_visible_entries(None, cx);
                                    cx.notify();
                                }
                            }
                        }
                    } else {
                    }
                }
            }
        }
    }

    fn collapse_active_entry(&mut self, _: &CollapseActiveEntry, cx: &mut ViewContext<Self>) {
        if let Some(active_entry) = self.active_entry {
            let project = self.project.read(cx);
            if let Some(worktree) = project.worktree_for_id(active_entry.worktree_id) {
                let worktree = worktree.read(cx);
                if let Some(mut entry) = worktree.entry_for_id(active_entry.entry_id) {
                    if let Some(expanded_dir_ids) =
                        self.expanded_dir_ids.get_mut(&active_entry.worktree_id)
                    {
                        loop {
                            match expanded_dir_ids.binary_search(&entry.id) {
                                Ok(ix) => {
                                    expanded_dir_ids.remove(ix);
                                    self.update_visible_entries(
                                        Some((active_entry.worktree_id, entry.id)),
                                        cx,
                                    );
                                    cx.notify();
                                    break;
                                }
                                Err(_) => {
                                    if let Some(parent_entry) =
                                        entry.path.parent().and_then(|p| worktree.entry_for_path(p))
                                    {
                                        entry = parent_entry;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn toggle_expanded(&mut self, action: &ToggleExpanded, cx: &mut ViewContext<Self>) {
        let ProjectEntry {
            worktree_ix,
            entry_id,
        } = action.0;
        let worktree_id = self.project.read(cx).worktrees()[worktree_ix].id();
        if let Some(expanded_dir_ids) = self.expanded_dir_ids.get_mut(&worktree_id) {
            match expanded_dir_ids.binary_search(&entry_id) {
                Ok(ix) => {
                    expanded_dir_ids.remove(ix);
                }
                Err(ix) => {
                    expanded_dir_ids.insert(ix, entry_id);
                }
            }
            self.update_visible_entries(Some((worktree_id, entry_id)), cx);
            cx.focus_self();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, cx: &mut ViewContext<Self>) {
        if let Some(active_entry) = self.active_entry {
            if active_entry.index > 0 {
                let (worktree_id, entry) = self
                    .visible_entry_for_index(active_entry.index - 1, cx)
                    .unwrap();
                self.active_entry = Some(ActiveEntry {
                    worktree_id,
                    entry_id: entry.id,
                    index: active_entry.index - 1,
                });
                self.autoscroll();
                cx.notify();
            }
        } else {
            self.select_first(cx);
        }
    }

    fn select_next(&mut self, _: &SelectNext, cx: &mut ViewContext<Self>) {
        if let Some(active_entry) = self.active_entry {
            if let Some((worktree_id, entry)) =
                self.visible_entry_for_index(active_entry.index + 1, cx)
            {
                self.active_entry = Some(ActiveEntry {
                    worktree_id,
                    entry_id: entry.id,
                    index: active_entry.index + 1,
                });
                self.autoscroll();
                cx.notify();
            }
        } else {
            self.select_first(cx);
        }
    }

    fn select_first(&mut self, cx: &mut ViewContext<Self>) {
        if let Some(worktree) = self.project.read(cx).worktrees().first() {
            let worktree_id = worktree.id();
            let worktree = worktree.read(cx);
            if let Some(root_entry) = worktree.root_entry() {
                self.active_entry = Some(ActiveEntry {
                    worktree_id,
                    entry_id: root_entry.id,
                    index: 0,
                });
                self.autoscroll();
                cx.notify();
            }
        }
    }

    fn autoscroll(&mut self) {
        if let Some(active_entry) = self.active_entry {
            self.list.scroll_to(active_entry.index);
        }
    }

    fn visible_entry_for_index<'a>(
        &self,
        target_ix: usize,
        cx: &'a AppContext,
    ) -> Option<(usize, &'a worktree::Entry)> {
        let project = self.project.read(cx);
        let mut offset = None;
        let mut ix = 0;
        for (worktree_ix, visible_entries) in self.visible_entries.iter().enumerate() {
            if target_ix < ix + visible_entries.len() {
                let worktree = &project.worktrees()[worktree_ix];
                offset = Some((worktree, visible_entries[target_ix - ix]));
                break;
            } else {
                ix += visible_entries.len();
            }
        }

        offset.and_then(|(worktree, offset)| {
            let mut entries = worktree.read(cx).entries(false);
            entries.advance_to_offset(offset);
            Some((worktree.id(), entries.entry()?))
        })
    }

    fn update_visible_entries(
        &mut self,
        new_active_entry: Option<(usize, usize)>,
        cx: &mut ViewContext<Self>,
    ) {
        let worktrees = self.project.read(cx).worktrees();
        self.visible_entries.clear();

        let mut entry_ix = 0;
        for worktree in worktrees {
            let snapshot = worktree.read(cx).snapshot();
            let worktree_id = worktree.id();

            let expanded_dir_ids = match self.expanded_dir_ids.entry(worktree_id) {
                hash_map::Entry::Occupied(e) => e.into_mut(),
                hash_map::Entry::Vacant(e) => {
                    // The first time a worktree's root entry becomes available,
                    // mark that root entry as expanded.
                    if let Some(entry) = snapshot.root_entry() {
                        e.insert(vec![entry.id]).as_slice()
                    } else {
                        &[]
                    }
                }
            };

            let mut visible_worktree_entries = Vec::new();
            let mut entry_iter = snapshot.entries(false);
            while let Some(item) = entry_iter.entry() {
                visible_worktree_entries.push(entry_iter.offset());
                if let Some(new_active_entry) = new_active_entry {
                    if new_active_entry == (worktree.id(), item.id) {
                        self.active_entry = Some(ActiveEntry {
                            worktree_id,
                            entry_id: item.id,
                            index: entry_ix,
                        });
                    }
                } else if self.active_entry.map_or(false, |e| {
                    e.worktree_id == worktree_id && e.entry_id == item.id
                }) {
                    self.active_entry = Some(ActiveEntry {
                        worktree_id,
                        entry_id: item.id,
                        index: entry_ix,
                    });
                }

                entry_ix += 1;
                if expanded_dir_ids.binary_search(&item.id).is_err() {
                    if entry_iter.advance_to_sibling() {
                        continue;
                    }
                }
                entry_iter.advance();
            }
            self.visible_entries.push(visible_worktree_entries);
        }

        self.autoscroll();
    }

    fn expand_entry(&mut self, worktree_id: usize, entry_id: usize, cx: &mut ViewContext<Self>) {
        let project = self.project.read(cx);
        if let Some((worktree, expanded_dir_ids)) = project
            .worktree_for_id(worktree_id)
            .zip(self.expanded_dir_ids.get_mut(&worktree_id))
        {
            let worktree = worktree.read(cx);

            if let Some(mut entry) = worktree.entry_for_id(entry_id) {
                loop {
                    if let Err(ix) = expanded_dir_ids.binary_search(&entry.id) {
                        expanded_dir_ids.insert(ix, entry.id);
                    }

                    if let Some(parent_entry) =
                        entry.path.parent().and_then(|p| worktree.entry_for_path(p))
                    {
                        entry = parent_entry;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    fn for_each_visible_entry<C: ReadModel>(
        &self,
        range: Range<usize>,
        cx: &mut C,
        mut callback: impl FnMut(ProjectEntry, EntryDetails, &mut C),
    ) {
        let project = self.project.read(cx);
        let worktrees = project.worktrees().to_vec();
        let mut ix = 0;
        for (worktree_ix, visible_worktree_entries) in self.visible_entries.iter().enumerate() {
            if ix >= range.end {
                return;
            }
            if ix + visible_worktree_entries.len() <= range.start {
                ix += visible_worktree_entries.len();
                continue;
            }

            let end_ix = range.end.min(ix + visible_worktree_entries.len());
            let worktree = &worktrees[worktree_ix];
            let expanded_entry_ids = self
                .expanded_dir_ids
                .get(&worktree.id())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let snapshot = worktree.read(cx).snapshot();
            let root_name = OsStr::new(snapshot.root_name());
            let mut cursor = snapshot.entries(false);

            for ix in visible_worktree_entries[range.start.saturating_sub(ix)..end_ix - ix]
                .iter()
                .copied()
            {
                cursor.advance_to_offset(ix);
                if let Some(entry) = cursor.entry() {
                    let filename = entry.path.file_name().unwrap_or(root_name);
                    let details = EntryDetails {
                        filename: filename.to_string_lossy().to_string(),
                        depth: entry.path.components().count(),
                        is_dir: entry.is_dir(),
                        is_expanded: expanded_entry_ids.binary_search(&entry.id).is_ok(),
                        is_active: self.active_entry.map_or(false, |e| {
                            e.worktree_id == worktree.id() && e.entry_id == entry.id
                        }),
                    };
                    let entry = ProjectEntry {
                        worktree_ix,
                        entry_id: entry.id,
                    };
                    callback(entry, details, cx);
                }
            }
            ix = end_ix;
        }
    }

    fn render_entry(
        entry: ProjectEntry,
        details: EntryDetails,
        theme: &theme::ProjectPanel,
        cx: &mut ViewContext<Self>,
    ) -> ElementBox {
        let is_dir = details.is_dir;
        MouseEventHandler::new::<Self, _, _, _>(
            (entry.worktree_ix, entry.entry_id),
            cx,
            |state, _| {
                let style = if details.is_active {
                    &theme.active_entry
                } else if state.hovered {
                    &theme.hovered_entry
                } else {
                    &theme.entry
                };
                Label::new(details.filename, style.text.clone())
                    .contained()
                    .with_style(style.container)
                    .with_padding_left(theme.entry_base_padding + details.depth as f32 * 20.)
                    .boxed()
            },
        )
        .on_click(move |cx| {
            if is_dir {
                cx.dispatch_action(ToggleExpanded(entry))
            } else {
                cx.dispatch_action(Open(entry))
            }
        })
        .with_cursor_style(CursorStyle::PointingHand)
        .boxed()
    }
}

impl View for ProjectPanel {
    fn ui_name() -> &'static str {
        "ProjectPanel"
    }

    fn render(&mut self, _: &mut gpui::RenderContext<'_, Self>) -> gpui::ElementBox {
        let settings = self.settings.clone();
        let handle = self.handle.clone();
        UniformList::new(
            self.list.clone(),
            self.visible_entries
                .iter()
                .map(|worktree_entries| worktree_entries.len())
                .sum(),
            move |range, items, cx| {
                let theme = &settings.borrow().theme.project_panel;
                let this = handle.upgrade(cx).unwrap();
                this.update(cx.app, |this, cx| {
                    this.for_each_visible_entry(range.clone(), cx, |entry, details, cx| {
                        items.push(Self::render_entry(entry, details, theme, cx));
                    });
                })
            },
        )
        .contained()
        .with_style(self.settings.borrow().theme.project_panel.container)
        .boxed()
    }

    fn keymap_context(&self, _: &AppContext) -> keymap::Context {
        let mut cx = Self::default_keymap_context();
        cx.set.insert("menu".into());
        cx
    }
}

impl Entity for ProjectPanel {
    type Event = Event;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::test_app_state;
    use gpui::{TestAppContext, ViewHandle};
    use serde_json::json;
    use std::{collections::HashSet, path::Path};

    #[gpui::test]
    async fn test_visible_list(mut cx: gpui::TestAppContext) {
        let app_state = cx.update(test_app_state);
        let settings = app_state.settings.clone();
        let fs = app_state.fs.as_fake();

        fs.insert_tree(
            "/root1",
            json!({
                ".dockerignore": "",
                ".git": {
                    "HEAD": "",
                },
                "a": {
                    "0": { "q": "", "r": "", "s": "" },
                    "1": { "t": "", "u": "" },
                    "2": { "v": "", "w": "", "x": "", "y": "" },
                },
                "b": {
                    "3": { "Q": "" },
                    "4": { "R": "", "S": "", "T": "", "U": "" },
                },
                "c": {
                    "5": {},
                    "6": { "V": "", "W": "" },
                    "7": { "X": "" },
                    "8": { "Y": {}, "Z": "" }
                }
            }),
        )
        .await;
        fs.insert_tree(
            "/root2",
            json!({
                "d": {
                    "9": ""
                },
                "e": {}
            }),
        )
        .await;

        let project = cx.add_model(|_| Project::new(&app_state));
        let root1 = project
            .update(&mut cx, |project, cx| {
                project.add_local_worktree("/root1".as_ref(), cx)
            })
            .await
            .unwrap();
        root1
            .read_with(&cx, |t, _| t.as_local().unwrap().scan_complete())
            .await;
        let root2 = project
            .update(&mut cx, |project, cx| {
                project.add_local_worktree("/root2".as_ref(), cx)
            })
            .await
            .unwrap();
        root2
            .read_with(&cx, |t, _| t.as_local().unwrap().scan_complete())
            .await;

        let (_, panel) = cx.add_window(|cx| ProjectPanel::new(project, settings, cx));
        assert_eq!(
            visible_entry_details(&panel, 0..50, &mut cx),
            &[
                EntryDetails {
                    filename: "root1".to_string(),
                    depth: 0,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false,
                },
                EntryDetails {
                    filename: ".dockerignore".to_string(),
                    depth: 1,
                    is_dir: false,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "a".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "b".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "c".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "root2".to_string(),
                    depth: 0,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false
                },
                EntryDetails {
                    filename: "d".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                },
                EntryDetails {
                    filename: "e".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                }
            ],
        );

        toggle_expand_dir(&panel, "root1/b", &mut cx);
        assert_eq!(
            visible_entry_details(&panel, 0..50, &mut cx),
            &[
                EntryDetails {
                    filename: "root1".to_string(),
                    depth: 0,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false,
                },
                EntryDetails {
                    filename: ".dockerignore".to_string(),
                    depth: 1,
                    is_dir: false,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "a".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "b".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false,
                },
                EntryDetails {
                    filename: "3".to_string(),
                    depth: 2,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "4".to_string(),
                    depth: 2,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "c".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false,
                },
                EntryDetails {
                    filename: "root2".to_string(),
                    depth: 0,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false
                },
                EntryDetails {
                    filename: "d".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                },
                EntryDetails {
                    filename: "e".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                }
            ]
        );

        assert_eq!(
            visible_entry_details(&panel, 5..8, &mut cx),
            [
                EntryDetails {
                    filename: "4".to_string(),
                    depth: 2,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                },
                EntryDetails {
                    filename: "c".to_string(),
                    depth: 1,
                    is_dir: true,
                    is_expanded: false,
                    is_active: false
                },
                EntryDetails {
                    filename: "root2".to_string(),
                    depth: 0,
                    is_dir: true,
                    is_expanded: true,
                    is_active: false
                }
            ]
        );

        fn toggle_expand_dir(
            panel: &ViewHandle<ProjectPanel>,
            path: impl AsRef<Path>,
            cx: &mut TestAppContext,
        ) {
            let path = path.as_ref();
            panel.update(cx, |panel, cx| {
                for (worktree_ix, worktree) in panel.project.read(cx).worktrees().iter().enumerate()
                {
                    let worktree = worktree.read(cx);
                    if let Ok(relative_path) = path.strip_prefix(worktree.root_name()) {
                        let entry_id = worktree.entry_for_path(relative_path).unwrap().id;
                        panel.toggle_expanded(
                            &ToggleExpanded(ProjectEntry {
                                worktree_ix,
                                entry_id,
                            }),
                            cx,
                        );
                        return;
                    }
                }
                panic!("no worktree for path {:?}", path);
            });
        }

        fn visible_entry_details(
            panel: &ViewHandle<ProjectPanel>,
            range: Range<usize>,
            cx: &mut TestAppContext,
        ) -> Vec<EntryDetails> {
            let mut result = Vec::new();
            let mut project_entries = HashSet::new();
            panel.update(cx, |panel, cx| {
                panel.for_each_visible_entry(range, cx, |project_entry, details, _| {
                    assert!(
                        project_entries.insert(project_entry),
                        "duplicate project entry {:?} {:?}",
                        project_entry,
                        details
                    );
                    result.push(details);
                });
            });

            result
        }
    }
}
