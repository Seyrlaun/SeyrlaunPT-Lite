use super::*;
impl<T> CompiledChoices<T> {
    /// All alternatives which can draw for this bound state, without random
    /// selection. Resource-dependent material tables must cover every choice.
    pub fn for_each_possible(&self, selected: &SelectedModels, mut f: impl FnMut(&T)) {
        let mut visit = |choice: &Choice<T>| match choice {
            Choice::Single(value) => f(value),
            Choice::Weighted { variants, .. } => variants.iter().for_each(&mut f),
        };
        match selected {
            SelectedModels::Variant(i) => {
                if let Some(choice) = self.variants.get(*i) {
                    visit(choice);
                }
            }
            SelectedModels::Multipart(indices) => {
                for i in indices {
                    if let Some(choice) = self.multipart.get(*i) {
                        visit(choice);
                    }
                }
            }
        }
    }
}
