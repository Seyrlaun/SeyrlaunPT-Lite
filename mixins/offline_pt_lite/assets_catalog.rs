use super::*;
use std::sync::{Arc, Mutex, OnceLock};
#[derive(Default)]
pub(crate) struct Extra {
    pub(crate) ctm: crate::ctm::ConnectedTextures,
    pub(crate) pbr: crate::pbr::PbrTextures,
}
type Entries = std::collections::HashMap<usize, Arc<Extra>>;
static EXTRAS: OnceLock<Mutex<Entries>> = OnceLock::new();
static EMPTY: OnceLock<Arc<Extra>> = OnceLock::new();
fn empty() -> Arc<Extra> {
    Arc::clone(EMPTY.get_or_init(|| Arc::new(Extra::default())))
}
fn entries() -> std::sync::MutexGuard<'static, Entries> {
    EXTRAS
        .get_or_init(|| Mutex::new(Entries::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
impl StateModels {
    pub(crate) fn face_keys(&self, state: BlockStateId) -> BTreeSet<(u32, u8)> {
        let mut result = BTreeSet::new();
        if let Some((choice, selected)) = self.state(state) {
            self.choices[*choice]
                .for_each_possible(
                    selected,
                    |model| {
                        for face in &self.baked[*model as usize].1.faces {
                            result.insert((face.layer, face.quad.facing as u8));
                        }
                    },
                );
        }
        result
    }
    fn native_pt_key(&self) -> Option<usize> {
        (!self.states.is_empty()).then(|| self.states.as_ptr() as usize)
    }
    pub(crate) fn native_pt_extra(&self) -> Arc<Extra> {
        let Some(key) = self.native_pt_key() else {
            return empty();
        };
        EXTRAS
            .get()
            .and_then(|table| {
                table
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&key)
                    .cloned()
            })
            .unwrap_or_else(empty)
    }
    pub(crate) fn native_pt_set_extra(&self, extra: Arc<Extra>) {
        let Some(key) = self.native_pt_key() else {
            assert!(
                extra.ctm.words.is_empty() && extra.ctm.neighbor_masks.is_empty() &&
                extra.pbr.words.is_empty() && extra.pbr.normal_count == 0 && extra.pbr
                .specular_count == 0,
                "an empty model catalog cannot own PT material data"
            );
            return;
        };
        entries().insert(key, extra);
    }
}
impl Drop for StateModels {
    fn drop(&mut self) {
        if let Some(key) = self.native_pt_key() && let Some(table) = EXTRAS.get() {
            table.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&key);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> StateModels {
        StateModels {
            choices: Vec::new(),
            states: vec![None],
            baked: Vec::new(),
        }
    }
    fn extra(normal_count: usize) -> Arc<Extra> {
        Arc::new(Extra {
            ctm: Default::default(),
            pbr: crate::pbr::PbrTextures {
                normal_count,
                ..Default::default()
            },
        })
    }
    #[test]
    fn loaded_catalogs_keep_independent_owned_materials() {
        let first = catalog();
        let second = catalog();
        let a = extra(11);
        let b = extra(22);
        first.native_pt_set_extra(Arc::clone(&a));
        second.native_pt_set_extra(Arc::clone(&b));
        assert!(Arc::ptr_eq(& first.native_pt_extra(), & a));
        assert!(Arc::ptr_eq(& second.native_pt_extra(), & b));
        assert_eq!(first.native_pt_extra().pbr.normal_count, 11);
        assert_eq!(second.native_pt_extra().pbr.normal_count, 22);
    }
    #[test]
    fn moving_a_catalog_keeps_its_materials_and_drop_releases_the_entry() {
        let catalog = catalog();
        let key = catalog.native_pt_key().unwrap();
        let materials = extra(33);
        let weak = Arc::downgrade(&materials);
        catalog.native_pt_set_extra(Arc::clone(&materials));
        let moved = Box::new(catalog);
        assert_eq!(moved.native_pt_key(), Some(key));
        assert!(Arc::ptr_eq(& moved.native_pt_extra(), & materials));
        drop(moved);
        assert!(
            entries().get(& key).is_none_or(| other | ! Arc::ptr_eq(other, & materials))
        );
        drop(materials);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn empty_catalogs_need_no_sidecar_entry() {
        let catalog = StateModels {
            choices: Vec::new(),
            states: Vec::new(),
            baked: Vec::new(),
        };
        catalog.native_pt_set_extra(Arc::new(Extra::default()));
        assert!(catalog.native_pt_key().is_none());
        assert!(catalog.native_pt_extra().pbr.words.is_empty());
    }
}
