use std::rc::Rc;

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    IndexPath,
    list::{ListDelegate, ListItem, ListState},
};
use gpui_kit::{App, Context, ParentElement, StatefulInteractiveElement, Window};
use jira_domain::{AccountId, User};

/// Keyboard navigable assignee choices used by the issue detail editor.
type ConfirmCallback = Rc<dyn Fn(&AssigneeChoice, &mut Window, &mut App)>;

pub(crate) struct AssigneeListDelegate {
    pub(crate) users: Vec<User>,
    selected: Option<usize>,
    on_confirm: ConfirmCallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssigneeChoice {
    pub(crate) account_id: Option<AccountId>,
    pub(crate) display_name: String,
}

impl AssigneeListDelegate {
    pub(crate) fn new(
        users: Vec<User>,
        on_confirm: impl Fn(&AssigneeChoice, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            users,
            selected: None,
            on_confirm: Rc::new(on_confirm),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_users(&mut self, users: Vec<User>) {
        self.users = users;
        self.selected = None;
    }

    pub(crate) fn choice_at(&self, index: usize) -> Option<AssigneeChoice> {
        if index == 0 {
            Some(AssigneeChoice {
                account_id: None,
                display_name: "Unassigned".to_owned(),
            })
        } else {
            self.users.get(index - 1).map(|user| AssigneeChoice {
                account_id: Some(user.account_id.clone()),
                display_name: user.display_name.clone(),
            })
        }
    }
}

impl ListDelegate for AssigneeListDelegate {
    type Item = ListItem;

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.users.len().saturating_add(1)
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let index = ix.row;
        let label = if index == 0 {
            "Unassigned".to_owned()
        } else {
            self.users.get(index - 1)?.display_name.clone()
        };
        let accessibility_id = format!("assignee-{index}");
        Some(
            ListItem::new(accessibility_id.clone())
                .accessibility_id(accessibility_id)
                .role(Role::ListBoxOption)
                .aria_label(label.clone())
                .aria_selected(self.selected == Some(index))
                .selected(self.selected == Some(index))
                .child(label),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix.map(|index| index.row);
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if let Some(index) = self.selected {
            let Some(choice) = self.choice_at(index) else {
                return;
            };
            (self.on_confirm)(&choice, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jira_domain::JiraSiteId;

    #[test]
    fn choice_at_preserves_account_identity_and_unassigned() {
        let user = User::new(
            JiraSiteId::new("site").expect("valid test site id"),
            AccountId::new("account-1").expect("valid test account id"),
            "Ada",
            None,
            true,
        );
        let delegate = AssigneeListDelegate::new(vec![user], |_, _, _| {});
        assert_eq!(
            delegate
                .choice_at(0)
                .expect("unassigned row should exist")
                .display_name,
            "Unassigned"
        );
        assert_eq!(
            delegate
                .choice_at(1)
                .expect("first user row should exist")
                .account_id
                .expect("first user should have an account id")
                .as_str(),
            "account-1"
        );
        assert!(delegate.choice_at(2).is_none());
    }

    #[test]
    fn refreshing_users_invalidates_selection_and_out_of_range_is_empty() {
        let mut delegate = AssigneeListDelegate::new(Vec::new(), |_, _, _| {});
        delegate.selected = Some(3);
        delegate.set_users(Vec::new());
        assert_eq!(delegate.selected, None);
        assert!(delegate.choice_at(1).is_none());
    }
}
