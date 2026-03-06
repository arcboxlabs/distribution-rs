pub mod m20250305_000001_init;
pub mod m20250305_000002_add_tenants;
pub mod m20250306_000001_extend_referrers;
pub mod m20250306_000002_add_gc_jobs;

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20250305_000001_init::Migration),
            Box::new(m20250305_000002_add_tenants::Migration),
            Box::new(m20250306_000001_extend_referrers::Migration),
            Box::new(m20250306_000002_add_gc_jobs::Migration),
        ]
    }
}
