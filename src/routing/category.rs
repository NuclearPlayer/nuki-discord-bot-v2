#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// A question about Nuclear, a report of a problem, or a request for a feature or a change.
    Help,
    Chat,
    Ignore,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::Help, Category::Chat, Category::Ignore];

    pub fn name(self) -> &'static str {
        match self {
            Category::Help => "help",
            Category::Chat => "chat",
            Category::Ignore => "ignore",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_the_snake_case_name_of_each_variant() {
        let names = Category::ALL.map(Category::name);

        assert_eq!(names, ["help", "chat", "ignore"]);
    }

    #[test]
    fn from_name_returns_the_variant_of_each_name() {
        let parsed = ["help", "chat", "ignore"].map(Category::from_name);

        assert_eq!(
            parsed,
            [
                Some(Category::Help),
                Some(Category::Chat),
                Some(Category::Ignore),
            ]
        );
    }

    #[test]
    fn from_name_returns_none_for_an_unknown_name() {
        assert_eq!(Category::from_name("bug_report"), None);
    }
}
