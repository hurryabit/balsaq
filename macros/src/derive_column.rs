use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, Expr, ExprLit, ExprUnary, Fields, Lit, Result, UnOp};

pub(crate) fn expand(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;

    match &input.data {
        Data::Struct(s) => expand_newtype(name, s),
        Data::Enum(e) => expand_enum(name, e, &input.attrs),
        _ => Err(Error::new_spanned(
            name,
            "derive(Column) requires a single-field tuple struct or a C-like enum with #[repr(integer)]",
        )),
    }
}

fn expand_newtype(name: &syn::Ident, s: &syn::DataStruct) -> Result<TokenStream> {
    let inner_ty = match &s.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => &fields.unnamed[0].ty,
        _ => {
            return Err(Error::new_spanned(
                name,
                "derive(Column) requires a single-field tuple struct",
            ));
        }
    };

    Ok(quote! {
        impl ::rusqlite::types::ToSql for #name {
            fn to_sql(&self) -> ::rusqlite::Result<::rusqlite::types::ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl ::rusqlite::types::FromSql for #name {
            fn column_result(
                value: ::rusqlite::types::ValueRef<'_>,
            ) -> ::rusqlite::types::FromSqlResult<Self> {
                <#inner_ty as ::rusqlite::types::FromSql>::column_result(value).map(Self)
            }
        }

        impl ::balsaq::Column for #name {
            const SQL_TYPE: &'static str = <#inner_ty as ::balsaq::Column>::SQL_TYPE;
            const NULLABLE: bool = <#inner_ty as ::balsaq::Column>::NULLABLE;
        }
    })
}

fn expand_enum(
    name: &syn::Ident,
    e: &syn::DataEnum,
    attrs: &[syn::Attribute],
) -> Result<TokenStream> {
    require_integer_repr(name, attrs)?;

    if e.variants.is_empty() {
        return Err(Error::new_spanned(
            name,
            "derive(Column) requires at least one variant",
        ));
    }

    // Collect variants and their i64 discriminant values.
    let mut next: i64 = 0;
    let mut variants: Vec<(&syn::Ident, i64)> = Vec::new();

    for v in &e.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err(Error::new_spanned(
                &v.ident,
                "derive(Column) on an enum requires unit variants (no fields)",
            ));
        }
        let disc = match &v.discriminant {
            Some((_, expr)) => parse_int_expr(expr)?,
            None => next,
        };
        next = disc
            .checked_add(1)
            .ok_or_else(|| Error::new_spanned(name, "discriminant overflow"))?;
        variants.push((&v.ident, disc));
    }

    // SQL_TYPE includes a CHECK constraint with all known discriminant values.
    let in_list: String = variants
        .iter()
        .map(|(_, d)| d.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let sql_type = format!("INTEGER CHECK ({{col}} IN ({in_list}))");

    let match_arms = variants.iter().map(|(ident, disc)| {
        quote! { #disc => ::std::result::Result::Ok(Self::#ident) }
    });

    Ok(quote! {
        impl ::rusqlite::types::ToSql for #name {
            fn to_sql(&self) -> ::rusqlite::Result<::rusqlite::types::ToSqlOutput<'_>> {
                ::std::result::Result::Ok((*self as i64).into())
            }
        }

        impl ::rusqlite::types::FromSql for #name {
            fn column_result(
                value: ::rusqlite::types::ValueRef<'_>,
            ) -> ::rusqlite::types::FromSqlResult<Self> {
                match <i64 as ::rusqlite::types::FromSql>::column_result(value)? {
                    #(#match_arms,)*
                    n => ::std::result::Result::Err(
                        ::rusqlite::types::FromSqlError::OutOfRange(n)
                    ),
                }
            }
        }

        impl ::balsaq::Column for #name {
            const SQL_TYPE: &'static str = #sql_type;
            const NULLABLE: bool = false;
        }
    })
}

fn require_integer_repr(name: &syn::Ident, attrs: &[syn::Attribute]) -> Result<()> {
    const INT_TYPES: &[&str] = &[
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "isize", "usize",
    ];
    for attr in attrs {
        if attr.path().is_ident("repr") {
            if let Ok(ident) = attr.parse_args::<syn::Ident>() {
                if INT_TYPES.contains(&ident.to_string().as_str()) {
                    return Ok(());
                }
            }
        }
    }
    Err(Error::new_spanned(
        name,
        "derive(Column) on an enum requires #[repr(integer)], e.g. #[repr(i64)]",
    ))
}

fn parse_int_expr(expr: &Expr) -> Result<i64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(li), ..
        }) => li
            .base10_parse::<i64>()
            .map_err(|e| Error::new(li.span(), e)),
        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => {
            if let Expr::Lit(ExprLit {
                lit: Lit::Int(li), ..
            }) = expr.as_ref()
            {
                li.base10_parse::<i64>()
                    .map(|v| -v)
                    .map_err(|e| Error::new(li.span(), e))
            } else {
                Err(Error::new_spanned(
                    expr,
                    "discriminant must be an integer literal",
                ))
            }
        }
        _ => Err(Error::new_spanned(
            expr,
            "discriminant must be an integer literal",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(input: TokenStream) -> String {
        let tokens = expand(input).unwrap();
        let file: syn::File = syn::parse2(tokens).unwrap();
        prettyplease::unparse(&file)
    }

    fn run_err(input: TokenStream) -> String {
        expand(input).unwrap_err().to_string()
    }

    // Newtype tests

    #[test]
    fn newtype_generates_to_sql_and_from_sql() {
        insta::assert_snapshot!(run(quote! { pub struct UserId(i64); }));
    }

    #[test]
    fn newtype_inner_type_used_in_from_sql() {
        insta::assert_snapshot!(run(quote! { pub struct Name(String); }));
    }

    #[test]
    fn error_named_fields() {
        let err = run_err(quote! { pub struct Foo { x: i64 } });
        assert!(err.contains("single-field tuple struct"));
    }

    #[test]
    fn error_multiple_fields() {
        let err = run_err(quote! { pub struct Foo(i64, String); });
        assert!(err.contains("single-field tuple struct"));
    }

    // Enum tests

    #[test]
    fn enum_generates_all_three_impls() {
        insta::assert_snapshot!(run(quote! {
            #[repr(i64)]
            pub enum Status { Active = 1, Inactive = 2 }
        }));
    }

    #[test]
    fn enum_check_constraint_contains_discriminants() {
        insta::assert_snapshot!(run(quote! {
            #[repr(i64)]
            pub enum Kind { A = 0, B = 1, C = 5 }
        }));
    }

    #[test]
    fn enum_implicit_discriminants() {
        insta::assert_snapshot!(run(quote! {
            #[repr(i64)]
            pub enum Tri { X, Y, Z }
        }));
    }

    #[test]
    fn error_enum_missing_repr() {
        let err = run_err(quote! { pub enum Foo { A, B } });
        assert!(err.contains("#[repr(integer)]"));
    }

    #[test]
    fn error_enum_non_unit_variant() {
        let err = run_err(quote! {
            #[repr(i64)]
            pub enum Foo { A = 0, B(i64) = 1 }
        });
        assert!(err.contains("unit variants"));
    }

    #[test]
    fn error_enum_non_literal_discriminant() {
        let err = run_err(quote! {
            #[repr(i64)]
            pub enum Foo { A = SOME_CONST }
        });
        assert!(err.contains("integer literal"));
    }
}
