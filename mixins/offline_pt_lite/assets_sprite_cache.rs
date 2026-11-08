use super::*;
/// Paths, sizes and modification times of every file under `assets/*/textures`,
/// summed so the directory order does not matter.
pub(super) fn native_pt_fingerprint(root: &Path) -> Option<u64> {
    fn walk(dir: &Path, base: &Path, sum: &mut u64) -> Option<()> {
        for entry in fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            if meta.is_dir() {
                walk(&entry.path(), base, sum)?;
                continue;
            }
            let mut hasher = DefaultHasher::new();
            entry.path().strip_prefix(base).ok()?.hash(&mut hasher);
            meta.len().hash(&mut hasher);
            meta.modified().ok()?.hash(&mut hasher);
            *sum = sum.wrapping_add(hasher.finish());
        }
        Some(())
    }
    let mut sum = 0x4f46464c49544531_u64;
    for namespace in fs::read_dir(root.join("assets")).ok()? {
        let namespace = namespace.ok()?.path();
        let textures = namespace.join("textures");
        if textures.is_dir() {
            walk(&textures, root, &mut sum)?;
        }
        let ctm = namespace.join("optifine/ctm");
        if ctm.is_dir() {
            walk(&ctm, root, &mut sum)?;
        }
    }
    Some(sum)
}
impl SpriteCache {
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}
