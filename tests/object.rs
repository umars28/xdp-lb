#[test]
fn compiled_object_is_parseable_by_the_loader() {
    match aya_obj::Object::parse(xdp_lb::object::bytes()) {
        Ok(_) => {}
        Err(err) => panic!("loader cannot parse the compiled object: {err:?}"),
    }
}

#[test]
fn object_exposes_the_xdp_program_and_every_map() {
    let object = aya_obj::Object::parse(xdp_lb::object::bytes()).expect("object must parse");

    assert!(
        object.programs.contains_key("xdp_lb"),
        "programs present: {:?}",
        object.programs.keys().collect::<Vec<_>>()
    );

    for name in [
        "services",
        "backends",
        "maglev",
        "conntrack",
        "stats",
        "backend_stats",
    ] {
        assert!(
            object.maps.contains_key(name),
            "map {name} missing; present: {:?}",
            object.maps.keys().collect::<Vec<_>>()
        );
    }
}
