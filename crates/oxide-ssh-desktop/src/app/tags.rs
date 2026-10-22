//! Tag registry UI state: the rows of the "manage tags" dialog and the
//! rename/recolor/delete operations they drive.

use gpui_component::input::InputEvent;

use super::*;

#[derive(Clone)]
pub(super) struct TagManagerEntry {
    pub(super) id: TagId,
    pub(super) name: Entity<InputState>,
    pub(super) color: Option<TagColor>,
    /// Whether the row's color swatches are visible; at most one row expands.
    pub(super) expanded: bool,
}

pub(super) struct TagManager {
    pub(super) entries: Vec<TagManagerEntry>,
    /// Inline "create tag" field above the registry rows.
    pub(super) new_tag: Entity<InputState>,
    /// Renames commit when a name field loses focus; dropping these would
    /// detach the handlers.
    _subscriptions: Vec<gpui::Subscription>,
}

impl AppView {
    pub(super) fn open_tag_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tag_manager.is_some() {
            return;
        }
        let tags: Vec<Tag> = match self.state.as_ref() {
            Some(state) => state.tags().into_iter().cloned().collect(),
            None => return,
        };
        let locale = self.locale;
        let new_tag = new_input(window, cx, MessageId::NewTag, "", false, locale);
        let mut entries = Vec::with_capacity(tags.len());
        let mut subscriptions = Vec::with_capacity(tags.len());
        for tag in tags {
            let name = new_input(window, cx, MessageId::Name, &tag.name, false, locale);
            let id = tag.id;
            subscriptions.push(cx.subscribe_in(
                &name,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur) {
                        this.commit_tag_name(id, window, cx);
                    }
                },
            ));
            entries.push(TagManagerEntry {
                id,
                name,
                color: tag.color,
                expanded: false,
            });
        }
        self.tag_manager = Some(TagManager {
            entries,
            new_tag,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    /// Closes the manager, committing any pending rename first.
    pub(super) fn close_tag_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_tag_names(window, cx);
        self.tag_manager = None;
        cx.notify();
    }

    pub(super) fn commit_tag_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<TagId> = self
            .tag_manager
            .as_ref()
            .map(|manager| manager.entries.iter().map(|entry| entry.id).collect())
            .unwrap_or_default();
        for id in ids {
            self.commit_tag_name(id, window, cx);
        }
    }

    fn commit_tag_name(&mut self, id: TagId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .tag_manager
            .as_ref()
            .and_then(|manager| manager.entries.iter().find(|entry| entry.id == id))
        else {
            return;
        };
        let input = entry.name.clone();
        let typed = input.read(cx).value().trim().to_string();
        let color = entry.color;
        if typed.is_empty() {
            return;
        }
        self.apply_tag_update(id, typed, color, window, cx);
    }

    pub(super) fn set_tag_color(
        &mut self,
        id: TagId,
        color: Option<TagColor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .tag_manager
            .as_ref()
            .and_then(|manager| manager.entries.iter().find(|entry| entry.id == id))
        else {
            return;
        };
        let typed = entry.name.read(cx).value().trim().to_string();
        let name = if typed.is_empty() {
            // An in-progress rename that was cleared keeps the stored name.
            match self.state.as_ref().and_then(|state| state.tag(id)) {
                Some(tag) => tag.name.clone(),
                None => return,
            }
        } else {
            typed
        };
        self.apply_tag_update(id, name, color, window, cx);
    }

    /// Creates a tag from the manager's inline field; the new row appears in
    /// the open dialog. Returns the entry so the dialog snapshot can update.
    pub(super) fn create_tag_from_manager(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TagManagerEntry> {
        let name = self
            .tag_manager
            .as_ref()
            .map(|manager| manager.new_tag.read(cx).value().trim().to_string())?;
        if name.is_empty() {
            return None;
        }
        let tag = match self
            .state
            .as_mut()
            .map(|state| state.create_tag(name, None))
        {
            Some(Ok(tag)) => tag,
            Some(Err(error)) => {
                self.status_message = Some(tag_storage_error_message_id(&error));
                cx.notify();
                return None;
            }
            None => return None,
        };
        let id = tag.id;
        let input = new_input(window, cx, MessageId::Name, &tag.name, false, self.locale);
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.commit_tag_name(id, window, cx);
                }
            },
        );
        let entry = TagManagerEntry {
            id,
            name: input,
            color: tag.color,
            expanded: false,
        };
        if let Some(manager) = self.tag_manager.as_mut() {
            manager.entries.push(entry.clone());
            manager
                .entries
                .sort_by(|left, right| {
                    left.name
                        .read(cx)
                        .value()
                        .to_lowercase()
                        .cmp(&right.name.read(cx).value().to_lowercase())
                });
            manager._subscriptions.push(subscription);
            manager
                .new_tag
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        cx.notify();
        Some(entry)
    }

    /// Expands one row's color swatches, collapsing the rest; clicking the
    /// expanded row's dot collapses it.
    pub(super) fn toggle_tag_expanded(&mut self, id: TagId, cx: &mut Context<Self>) {
        if let Some(manager) = self.tag_manager.as_mut() {
            for entry in &mut manager.entries {
                entry.expanded = entry.id == id && !entry.expanded;
            }
        }
        cx.notify();
    }

    pub(super) fn request_tag_delete(&mut self, id: TagId, cx: &mut Context<Self>) {
        let in_use = self
            .state
            .as_ref()
            .is_some_and(|state| !state.profiles_with_tag(id).is_empty());
        if in_use {
            self.confirm = Some(ConfirmAction::DeleteTag(id));
            cx.notify();
            return;
        }
        self.delete_tag(id, cx);
    }

    /// Deletes an unreferenced (or confirmed) tag and prunes it from open
    /// dialogs.
    pub(super) fn delete_tag(&mut self, id: TagId, cx: &mut Context<Self>) {
        match self.state.as_mut().map(|state| state.delete_tag(id)) {
            Some(Ok(())) => self.handle_tag_deleted(id),
            Some(Err(error)) => self.status_message = Some(tag_storage_error_message_id(&error)),
            None => {}
        }
        cx.notify();
    }

    /// Drops a deleted tag from the open manager and from any open editor form.
    pub(super) fn handle_tag_deleted(&mut self, id: TagId) {
        if let Some(manager) = &mut self.tag_manager {
            manager.entries.retain(|entry| entry.id != id);
        }
        if let Some(editor) = &mut self.editor {
            editor.form.tag_ids.retain(|tag_id| *tag_id != id);
        }
    }

    fn apply_tag_update(
        &mut self,
        id: TagId,
        name: String,
        color: Option<TagColor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self
            .state
            .as_ref()
            .and_then(|state| state.tag(id))
            .is_some_and(|tag| tag.name == name && tag.color == color);
        if unchanged {
            return;
        }
        let result = self
            .state
            .as_mut()
            .map(|state| state.update_tag(id, name.clone(), color));
        match result {
            Some(Ok(())) => {
                if let Some(entry) = self
                    .tag_manager
                    .as_mut()
                    .and_then(|manager| manager.entries.iter_mut().find(|entry| entry.id == id))
                {
                    entry.color = color;
                    let input = entry.name.clone();
                    // Keep the field showing exactly what was persisted.
                    if input.read(cx).value().trim() != name.as_str() {
                        let value = name.clone();
                        input.update(cx, |field, cx| field.set_value(value, window, cx));
                    }
                }
            }
            Some(Err(error)) => {
                self.status_message = Some(tag_storage_error_message_id(&error));
                let persisted = self
                    .state
                    .as_ref()
                    .and_then(|state| state.tag(id))
                    .map(|tag| tag.name.clone());
                if let (Some(persisted), Some(entry)) = (
                    persisted,
                    self.tag_manager
                        .as_ref()
                        .and_then(|manager| manager.entries.iter().find(|entry| entry.id == id)),
                ) {
                    entry
                        .name
                        .update(cx, |input, cx| input.set_value(persisted, window, cx));
                }
            }
            None => {}
        }
        cx.notify();
    }
}
