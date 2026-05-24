use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Item, Result};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    if !attr.is_empty() {
        return Err(Error::new_spanned(attr, "#[schema] takes no arguments"));
    }

    let mut module: syn::ItemMod = syn::parse2(item)?;

    let Some((_, ref mut items)) = module.content else {
        return Err(Error::new_spanned(
            &module.ident,
            "#[schema] requires an inline module; `mod name;` form is not supported",
        ));
    };

    let table_structs: Vec<syn::Ident> = items
        .iter()
        .filter_map(|item| {
            if let Item::Struct(s) = item {
                s.attrs
                    .iter()
                    .any(|a| {
                        a.path()
                            .segments
                            .last()
                            .is_some_and(|seg| seg.ident == "table")
                    })
                    .then(|| s.ident.clone())
            } else {
                None
            }
        })
        .collect();

    if table_structs.is_empty() {
        return Err(Error::new_spanned(
            &module.ident,
            "#[schema] module contains no #[table] structs",
        ));
    }

    let schema_item: Item = syn::parse2(quote! {
        pub const SCHEMA: &'static str = ::balsaq::__cf::concatcp!(
            #(<#table_structs as ::balsaq::Model>::CREATE_TABLE),*
        );
    })?;
    items.push(schema_item);

    Ok(quote! { #module })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(attr: TokenStream, item: TokenStream) -> String {
        let tokens = expand(attr, item).unwrap();
        let file: syn::File = syn::parse2(tokens).unwrap();
        prettyplease::unparse(&file)
    }

    fn run_err(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap_err().to_string()
    }

    #[test]
    fn single_table_generates_schema() {
        insta::assert_snapshot!(run(
            quote! {},
            quote! {
                mod db {
                    #[table("widgets")]
                    pub struct Widget {}
                }
            },
        ));
    }

    #[test]
    fn multiple_tables_all_appear_in_schema() {
        insta::assert_snapshot!(run(
            quote! {},
            quote! {
                mod db {
                    #[table("widgets")]
                    pub struct Widget {}
                    #[table("posts")]
                    pub struct Post {}
                }
            },
        ));
    }

    #[test]
    fn non_table_items_are_passed_through() {
        insta::assert_snapshot!(run(
            quote! {},
            quote! {
                mod db {
                    pub struct Helper;
                    #[column_set]
                    pub struct Info {}
                    #[table("t")]
                    pub struct T {}
                }
            },
        ));
    }

    #[test]
    fn error_no_table_structs() {
        let err = run_err(
            quote! {},
            quote! {
                mod db {
                    pub struct NotATable;
                }
            },
        );
        assert!(err.contains("no #[table] structs"));
    }

    #[test]
    fn error_non_inline_module() {
        let err = run_err(quote! {}, quote! { mod db; });
        assert!(err.contains("inline module"));
    }

    #[test]
    fn error_attr_args_rejected() {
        let err = run_err(quote! { something }, quote! { mod db {} });
        assert!(err.contains("takes no arguments"));
    }
}
