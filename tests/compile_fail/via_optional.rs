use balsaq::{group, table};

#[group]
pub struct Raw {
    pub kind: i64,
}

#[table("things")]
pub struct Thing {
    #[primary_key]
    pub id: i64,
    #[group(via = Raw, extra)]
    pub value: i64,
}

fn main() {}
