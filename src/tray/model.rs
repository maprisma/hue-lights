#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Activate,
    Open,
    Quit,
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
        item(1, "Open Maprisma Hue Lights", Some(Action::Open)),
        item(2, "Quit tray", Some(Action::Quit)),
    ];
    root
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_actions() {
        let m = menu();
        assert_eq!(m.children.len(), 2);
        assert_eq!(m.find(1).unwrap().label, "Open Maprisma Hue Lights");
        assert_eq!(m.activate(1), Some(Action::Open));
        assert_eq!(m.activate(2), Some(Action::Quit));
        assert_eq!(m.activate(0), None);
        assert_eq!(m.activate(99), None);
    }
}
