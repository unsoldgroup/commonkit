use commonkit_adapters::{PackageAdapter, PackageMutationBackend};

#[test]
fn package_adapter_api_exposes_only_the_typed_offline_backend_seam() {
    fn assert_send<T: Send>() {}

    assert_send::<PackageAdapter>();
    let _typed_backend: Option<Box<dyn PackageMutationBackend>> = None;
}
