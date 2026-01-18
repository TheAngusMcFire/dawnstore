#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct EmptyObject {}

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub enum CarModel {
    VW,
    BMW,
    SEAD,
    Jeep,
}

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct TestCar {
    pub ps: u32,
    #[validate(range(min = 1900, max = 2100))]
    pub year: u32,
    pub brand: String,
    pub model: CarModel,
    pub items: Vec<String>,
}
