pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}

impl<T: ColumnGroup> ColumnGroup for Option<T> {
    // {NN} substitution is handled by the outer #[table] macro, which reads
    // NULLABLE = true and replaces {NN} with "" (making columns nullable).
    const DDL: &'static str = T::DDL;
    const COLS: &'static str = T::COLS;
    const VALS: &'static str = T::VALS;
    const NULLABLE: bool = true;

    fn cg_read(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Self> {
        T::cg_read_optional(row, prefix)
    }

    fn cg_write<'a>(&'a self, prefix: &str, out: &mut Vec<(String, &'a dyn ToSql)>) {
        match self {
            Some(t) => t.cg_write(prefix, out),
            None => T::cg_write_null(prefix, out),
        }
    }

    fn cg_read_optional(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Option<Self>> {
        T::cg_read_optional(row, prefix).map(Some)
    }

    fn cg_write_null<'a>(prefix: &str, out: &mut Vec<(String, &'a dyn ToSql)>) {
        T::cg_write_null(prefix, out)
    }
}
