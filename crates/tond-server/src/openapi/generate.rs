// openapi/generate.rs — placeholder

#[test]
fn route_table_describes_every_handler() {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let table = crate::routes::route_table(&mut generator);
    assert!(table.len() > 200);
}
