#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct EmptyObject {}

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct TestCar {
    pub ps: u32,
    pub year: u32,
    pub brand: String,
    pub model: String,
}
