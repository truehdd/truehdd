use darling::Error;
use darling::ast::NestedMeta;
use quote::quote;
use syn::{Data, DeriveInput, Fields, ItemStruct, parse_macro_input};

use proc_macro::TokenStream;

#[proc_macro_derive(ToBytes)]
pub fn derive_to_bytes(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let fields: Vec<syn::Member> = match input.data {
        Data::Struct(ref s) => match s.fields {
            Fields::Named(ref nf) => nf
                .named
                .iter()
                .map(|f| f.ident.clone().unwrap().into())
                .collect(),
            Fields::Unnamed(ref uf) => uf
                .unnamed
                .iter()
                .enumerate()
                .map(|(i, _)| syn::Index::from(i).into())
                .collect(),
            Fields::Unit => Vec::new(),
        },
        _ => unreachable!("ToBytes can only be derived for structs"),
    };

    let expanded = quote! {
        impl crate::byteorder::WriteBytesBe for #name {
            fn write_be(&self, dst: &mut Vec<u8>) {
                #( crate::byteorder::WriteBytesBe::write_be(&self.#fields, dst); )*
            }
        }

        impl crate::byteorder::WriteBytesLe for #name {
            fn write_le(&self, dst: &mut Vec<u8>) {
                #( crate::byteorder::WriteBytesLe::write_le(&self.#fields, dst); )*
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn caf_chunk_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match NestedMeta::parse_meta_list(attr.into()) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(Error::from(e).write_errors());
        }
    };

    let type_bytes = match &args[0] {
        NestedMeta::Lit(syn::Lit::ByteStr(bs)) => bs.value(),
        _ => panic!("chunk_type expects a byte string, e.g. b\"desc\""),
    };

    if type_bytes.len() != 4 {
        return TokenStream::from(
            syn::Error::new_spanned(&args[0], "chunk_type expects 4 bytes").to_compile_error(),
        );
    }
    let type_bytes_tokens = {
        let b = type_bytes;
        quote! {[#(#b),*]}
    };

    let input = parse_macro_input!(item as ItemStruct);
    let name = &input.ident;

    let expanded = quote! {
        #input

        impl CAFChunk for #name {
            fn chunk_type(&self) -> &[u8; 4] {
                const BYTES: [u8; 4] = #type_bytes_tokens;
                &BYTES
            }

            fn chunk_data(&self) -> Vec<u8> {
                let mut vec = Vec::new();
                self.write_be(&mut vec);
                vec
            }
        }
    };
    TokenStream::from(expanded)
}
