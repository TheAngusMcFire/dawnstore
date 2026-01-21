use dawnstore_lib::ReturnObject;

pub struct ForeignKey {
    pub path: String,
    pub parent_path: Option<String>,
    pub ty: ForeignKeyType,
    pub behaviour: ForeignKeyBehaviour,
    /// None: different kinds are allowed
    pub foreign_kind: Option<String>,
}
impl ForeignKey {
    pub fn new(
        path: impl Into<String>,
        parent_path: Option<impl Into<String>>,
        ty: ForeignKeyType,
        foreign_kind: Option<impl Into<String>>,
    ) -> Self {
        Self {
            path: path.into(),
            ty,
            behaviour: ForeignKeyBehaviour::Fill,
            foreign_kind: foreign_kind.map(|x| x.into()),
            parent_path: parent_path.map(|x| x.into()),
        }
    }
}

#[derive(Debug, sqlx::Type, Clone, PartialEq, Eq)]
#[sqlx(type_name = "foreign_key_type", rename_all = "PascalCase")]
pub enum ForeignKeyType {
    One,
    OneOptional,
    OneOrMany,
    NoneOrMany,
}

#[derive(Debug, sqlx::Type, Clone)]
#[sqlx(type_name = "foreign_key_behaviour", rename_all = "PascalCase")]
pub enum ForeignKeyBehaviour {
    Fill,
    Ignore,
}

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

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Container {
    pub nr: u32,
    pub notes: Option<String>,
    pub parent: Option<String>,
    #[schemars(skip)]
    pub parent_object: Option<ReturnObject<Box<Container>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub items: Vec<String>,
}
