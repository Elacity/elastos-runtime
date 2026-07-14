wit_bindgen::generate!({
    path: "../../../elastos/wit",
    world: "product-capsule-v1",
});

struct ExampleComponentApp;

impl exports::elastos::bus::lifecycle::Guest for ExampleComponentApp {
    fn run() -> Result<(), elastos::bus::types::BusError> {
        let _runtime = elastos::bus::runtime::info();
        let _identity = elastos::bus::identity::context();
        Ok(())
    }
}

export!(ExampleComponentApp);
