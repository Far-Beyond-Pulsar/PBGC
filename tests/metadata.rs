use graphy::NodeMetadataProvider;

#[test]
fn the_standard_library_nodes_are_available_as_graphy_metadata() {
    let metadata = pbgc::extract_node_metadata().expect("pulsar_std registry");
    assert!(metadata.len() > 100, "the standard library registers its nodes ({} found)", metadata.len());
    for name in ["add", "branch", "do_once", "delay"] {
        assert!(metadata.contains_key(name), "`{name}` is in the catalogue");
    }
}

#[test]
fn flow_intrinsics_keep_their_exec_pins() {
    let node = pbgc::find_node("do_once").expect("do_once");
    assert_eq!(node.exec_outputs, ["Then"]);
    let delay = pbgc::find_node("delay").expect("delay");
    assert_eq!(delay.exec_outputs, ["Completed"]);
}

#[test]
fn the_provider_serves_the_same_catalogue() {
    let provider = pbgc::BlueprintMetadataProvider::new();
    assert!(provider.get_node_metadata("add").is_some());
    assert!(provider.get_node_metadata("no_such_node").is_none());
    assert_eq!(provider.get_all_nodes().len(), pbgc::get_node_metadata().len());
    assert!(!pbgc::get_categories().is_empty());
}
