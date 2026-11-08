fn register_mod(game: &mut Game<'_>, mods: &mut Vec<NativeMod>) {
    offline_pt_native_ui::register(game, mods);
}
