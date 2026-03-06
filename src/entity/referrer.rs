use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "referrers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub repo_id: i32,
    #[sea_orm(primary_key, auto_increment = false)]
    pub subject_digest: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub referrer_digest: String,
    pub artifact_type: Option<String>,
    pub media_type: String,
    pub size: i64,
    pub annotations: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::repository::Entity",
        from = "Column::RepoId",
        to = "super::repository::Column::Id"
    )]
    Repository,
}

impl Related<super::repository::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repository.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
