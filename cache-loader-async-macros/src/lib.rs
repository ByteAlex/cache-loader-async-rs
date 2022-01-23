extern crate proc_macro;

use quote::quote;
use proc_macro2::{Ident, Span, TokenStream, TokenTree};
use proc_macro2::token_stream::IntoIter;

fn collect_diamond_idents(stream: &mut IntoIter) -> Vec<Ident> {
    let mut idents = Vec::new();
    if let TokenTree::Punct(punct) = stream.next().expect("Missing next element") {
        if !punct.as_char().eq(&'<') {
            panic!("Invalid diamond start");
        }
    } else {
        panic!("Invalid diamond start");
    }
    let mut expect_ident = true;
    loop {
        match stream.next().expect("Missing next element") {
            TokenTree::Ident(ident) => {
                if expect_ident {
                    expect_ident = false;
                    idents.push(ident);
                } else {
                    panic!("Invalid diamond format! (Didn't expect ident)");
                }
            },
            TokenTree::Punct(punct) => {
                if !expect_ident {
                    if punct.as_char().eq(&',') {
                        expect_ident = true;
                    } else if punct.as_char().eq(&'>') {
                        break;
                    } else {
                        panic!("Invalid diamond format! (Invalid punct)");
                    }
                } else {
                    panic!("Invalid diamond format! (Didn't expect punct)");
                }
            }
            _ => panic!("Invalid type"),
        }
    }
    idents
}

#[proc_macro]
pub fn test_with_features(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let mut stream = TokenStream::from(item).into_iter();
    let fn_ident = if let TokenTree::Ident(ident) = stream.next().expect("First token mandatory") {
        ident
    } else {
        panic!("First token must be an ident!");
    };
    let ident = if let TokenTree::Ident(ident) = stream.next().expect("Second token mandatory") {
        ident
    } else {
        panic!("Second token must be an ident!");
    };
    let types = collect_diamond_idents(&mut stream);
    let loader = if let TokenTree::Group(group) = stream.next().expect("Missing group token") {
        group
    } else {
        panic!("Group token not present");
    };

    let mut fn_body = quote! {};
    while let Some(token) = stream.next() {
        fn_body = quote! {
            #fn_body #token
        }
    }

    let key_type = types.get(0).unwrap();
    let value_type = types.get(1).unwrap();
    let error_type = types.get(2).unwrap();

    let fn_ident_default = syn::Ident::new(&format!("test_default_{}", fn_ident), Span::call_site());
    let fn_ident_lru = syn::Ident::new(&format!("test_lru_{}", fn_ident), Span::call_site());
    let fn_ident_ttl = syn::Ident::new(&format!("test_ttl_{}", fn_ident), Span::call_site());

    let result = quote! {
        #[tokio::test]
        async fn #fn_ident_default() {
            let #ident: LoadingCache<#key_type, #value_type, #error_type, HashMapBacking<_, _>> = LoadingCache::new(move |key: #key_type| {
               async move #loader
            });

            #fn_body
        }

        #[cfg(feature = "lru-cache")]
        #[tokio::test]
        async fn #fn_ident_lru() {
            let #ident: LoadingCache<#key_type, #value_type, #error_type, LruCacheBacking<_, _>> = LoadingCache::with_backing(LruCacheBacking::new(100), move |key: #key_type| {
               async move #loader
            });

            #fn_body
        }

        #[cfg(feature = "ttl-cache")]
        #[tokio::test]
        async fn #fn_ident_ttl() {
            let #ident: LoadingCache<#key_type, #value_type, #error_type, TtlCacheBacking<_, _>> = LoadingCache::with_backing(TtlCacheBacking::new(Duration::from_secs(3)), move |key: #key_type| {
               async move #loader
            });

            #fn_body
        }
    };

    return result.into();
}