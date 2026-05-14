use balsaq::table;

#[table("things")]
#[index(nonexistent)]
pub struct Thing {
    pub name: String,
}

fn main() {}
