use balsaq::{group, table};

#[group]
pub struct AuthorInfo {
    pub name: String,
}

// `author` is a #[group] field — indexing it should be rejected.
#[table("posts")]
#[index(author)]
pub struct Post {
    #[primary_key]
    pub id: Vec<u8>,
    #[group]
    pub author: AuthorInfo,
}

fn main() {}
