use dawnstore_lib::ReturnObject;

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
    pub year: u32,
    pub brand: String,
    pub model: CarModel,
    pub items: Vec<String>,
}

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Container {
    pub nr: u32,
    pub notes: Option<String>,
    pub parent: Option<String>,
    pub parent_object: Option<ReturnObject<Box<Container>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub items: Vec<String>,
}
