fn mesh(
    mesher: &mut Mesher<'_>,
    region: &[[Option<&Chunk>; 3]; 3],
    sy: i32,
) -> ::core::ops::ControlFlow<SectionMesh> {
    ::core::ops::ControlFlow::Break(mesher.native_pt_mesh(region, sy))
}
