use std::fs;
use std::path::PathBuf;

use commonkit_contracts::StyleguideDescriptor;

#[test]
fn first_party_styleguide_descriptor_matches_the_typed_contract() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("packages/commonkit-styleguide/styleguide-descriptor.json");
    let descriptor: StyleguideDescriptor =
        serde_json::from_slice(&fs::read(path).expect("descriptor")).expect("typed descriptor");

    descriptor.validate().expect("valid descriptor");
    descriptor.digest().expect("digest");
}
