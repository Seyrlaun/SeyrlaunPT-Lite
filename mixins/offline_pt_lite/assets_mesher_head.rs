fn mesh(
    mesher: &mut Mesher<'_>,
    region: &[[Option<&Chunk>; 3]; 3],
    sy: i32,
    out: &mut SectionMesh,
    scratch: &mut MeshScratch,
) -> ::core::ops::ControlFlow<SectionMesh> {
    ::core::ops::ControlFlow::Break(
        mesher.native_pt_mesh_reusing(region, sy, std::mem::take(out), scratch),
    )
}
