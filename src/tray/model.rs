use maprisma_hue_lights::i18n::{tr, choices};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Activate,
    Open,
    Quit,
    Language(usize),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub children: Vec<Item>,
    action: Option<Action>,
}
impl Item {
    pub fn find(&self, id: i32) -> Option<&Self> {
        if self.id == id {
            Some(self)
        } else {
            self.children.iter().find_map(|c| c.find(id))
        }
    }
    pub fn activate(&self, id: i32) -> Option<Action> {
        self.find(id).filter(|i| i.enabled).and_then(|i| i.action)
    }
}
pub fn menu() -> Item {
    let item = |id, label: &str, action| Item {
        id,
        label: label.into(),
        enabled: true,
        separator: false,
        children: vec![],
        action,
    };
    let mut root = item(0, "", None);
    root.children = vec![
        item(1, &tr("Open Maprisma Hue Lights"), Some(Action::Open)),
        item(2, &tr("Quit tray"), Some(Action::Quit)),
    ];
    let mut languages = item(3, &tr("Language"), None);
    languages.children = choices().iter().enumerate().map(|(i, (_, label))| item(10 + i as i32, label, Some(Action::Language(i)))).collect();
    root.children.push(languages);
    root
}
pub fn menu_error() -> Item {
    let mut menu = menu();
    menu.children.push(Item { id: 30, label: tr("Could not save language setting."), enabled: false, separator: false, children: vec![], action: None });
    menu
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_actions() {
        let m = menu();
        assert_eq!(m.children.len(), 3);
        assert_eq!(m.find(1).unwrap().label, "Open Maprisma Hue Lights");
        assert_eq!(m.activate(1), Some(Action::Open));
        assert_eq!(m.activate(2), Some(Action::Quit));
        assert_eq!(m.activate(0), None);
        assert_eq!(m.activate(99), None);
    }
}
