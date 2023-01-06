use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::mem;
use syn::{
    parse_macro_input, parse_quote, spanned::Spanned as _, AttributeArgs, FnArg, ItemFn, Lit, Meta,
    NestedMeta, Type,
};

#[proc_macro_attribute]
pub fn test(args: TokenStream, function: TokenStream) -> TokenStream {
    let mut namespace = format_ident!("gpui");

    let args = syn::parse_macro_input!(args as AttributeArgs);
    let mut max_retries = 0;
    let mut num_iterations = 1;
    let mut starting_seed = 0;
    let mut detect_nondeterminism = false;

    for arg in args {
        match arg {
            NestedMeta::Meta(Meta::Path(name))
                if name.get_ident().map_or(false, |n| n == "self") =>
            {
                namespace = format_ident!("crate");
            }
            NestedMeta::Meta(Meta::NameValue(meta)) => {
                let key_name = meta.path.get_ident().map(|i| i.to_string());
                let result = (|| {
                    match key_name.as_deref() {
                        Some("detect_nondeterminism") => {
                            detect_nondeterminism = parse_bool(&meta.lit)?
                        }
                        Some("retries") => max_retries = parse_int(&meta.lit)?,
                        Some("iterations") => num_iterations = parse_int(&meta.lit)?,
                        Some("seed") => starting_seed = parse_int(&meta.lit)?,
                        _ => {
                            return Err(TokenStream::from(
                                syn::Error::new(meta.path.span(), "invalid argument")
                                    .into_compile_error(),
                            ))
                        }
                    }
                    Ok(())
                })();

                if let Err(tokens) = result {
                    return tokens;
                }
            }
            other => {
                return TokenStream::from(
                    syn::Error::new_spanned(other, "invalid argument").into_compile_error(),
                )
            }
        }
    }

    let mut inner_fn = parse_macro_input!(function as ItemFn);
    if max_retries > 0 && num_iterations > 1 {
        return TokenStream::from(
            syn::Error::new_spanned(inner_fn, "retries and randomized iterations can't be mixed")
                .into_compile_error(),
        );
    }
    let inner_fn_attributes = mem::take(&mut inner_fn.attrs);
    let inner_fn_name = format_ident!("_{}", inner_fn.sig.ident);
    let outer_fn_name = mem::replace(&mut inner_fn.sig.ident, inner_fn_name.clone());

    let mut outer_fn: ItemFn = if inner_fn.sig.asyncness.is_some() {
        // Pass to the test function the number of app contexts that it needs,
        // based on its parameter list.
        let mut cx_vars = proc_macro2::TokenStream::new();
        let mut cx_teardowns = proc_macro2::TokenStream::new();
        let mut inner_fn_args = proc_macro2::TokenStream::new();
        for (ix, arg) in inner_fn.sig.inputs.iter().enumerate() {
            if let FnArg::Typed(arg) = arg {
                if let Type::Path(ty) = &*arg.ty {
                    let last_segment = ty.path.segments.last();
                    match last_segment.map(|s| s.ident.to_string()).as_deref() {
                        Some("StdRng") => {
                            inner_fn_args.extend(quote!(rand::SeedableRng::seed_from_u64(seed),));
                            continue;
                        }
                        Some("Arc") => {
                            if let syn::PathArguments::AngleBracketed(args) =
                                &last_segment.unwrap().arguments
                            {
                                if let Some(syn::GenericArgument::Type(syn::Type::Path(ty))) =
                                    args.args.last()
                                {
                                    let last_segment = ty.path.segments.last();
                                    if let Some("Deterministic") =
                                        last_segment.map(|s| s.ident.to_string()).as_deref()
                                    {
                                        inner_fn_args.extend(quote!(deterministic.clone(),));
                                        continue;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                } else if let Type::Reference(ty) = &*arg.ty {
                    if let Type::Path(ty) = &*ty.elem {
                        let last_segment = ty.path.segments.last();
                        if let Some("TestAppContext") =
                            last_segment.map(|s| s.ident.to_string()).as_deref()
                        {
                            let first_entity_id = ix * 100_000;
                            let cx_varname = format_ident!("cx_{}", ix);
                            cx_vars.extend(quote!(
                                let mut #cx_varname = #namespace::TestAppContext::new(
                                    foreground_platform.clone(),
                                    cx.platform().clone(),
                                    deterministic.build_foreground(#ix),
                                    deterministic.build_background(),
                                    cx.font_cache().clone(),
                                    cx.leak_detector(),
                                    #first_entity_id,
                                    stringify!(#outer_fn_name).to_string(),
                                );
                            ));
                            cx_teardowns.extend(quote!(
                                #cx_varname.update(|cx| cx.remove_all_windows());
                                deterministic.run_until_parked();
                                #cx_varname.update(|cx| cx.clear_globals());
                            ));
                            inner_fn_args.extend(quote!(&mut #cx_varname,));
                            continue;
                        }
                    }
                }
            }

            return TokenStream::from(
                syn::Error::new_spanned(arg, "invalid argument").into_compile_error(),
            );
        }

        parse_quote! {
            #[test]
            fn #outer_fn_name() {
                #inner_fn

                #namespace::test::run_test(
                    #num_iterations as u64,
                    #starting_seed as u64,
                    #max_retries,
                    #detect_nondeterminism,
                    &mut |cx, foreground_platform, deterministic, seed| {
                        #cx_vars
                        cx.foreground().run(#inner_fn_name(#inner_fn_args));
                        #cx_teardowns
                    },
                    stringify!(#outer_fn_name).to_string(),
                );
            }
        }
    } else {
        let mut inner_fn_args = proc_macro2::TokenStream::new();
        for arg in inner_fn.sig.inputs.iter() {
            if let FnArg::Typed(arg) = arg {
                if let Type::Path(ty) = &*arg.ty {
                    let last_segment = ty.path.segments.last();

                    if let Some("StdRng") = last_segment.map(|s| s.ident.to_string()).as_deref() {
                        inner_fn_args.extend(quote!(rand::SeedableRng::seed_from_u64(seed),));
                    }
                } else {
                    inner_fn_args.extend(quote!(cx,));
                }
            } else {
                return TokenStream::from(
                    syn::Error::new_spanned(arg, "invalid argument").into_compile_error(),
                );
            }
        }

        parse_quote! {
            #[test]
            fn #outer_fn_name() {
                #inner_fn

                #namespace::test::run_test(
                    #num_iterations as u64,
                    #starting_seed as u64,
                    #max_retries,
                    #detect_nondeterminism,
                    &mut |cx, _, _, seed| #inner_fn_name(#inner_fn_args),
                    stringify!(#outer_fn_name).to_string(),
                );
            }
        }
    };
    outer_fn.attrs.extend(inner_fn_attributes);

    TokenStream::from(quote!(#outer_fn))
}

fn parse_int(literal: &Lit) -> Result<usize, TokenStream> {
    let result = if let Lit::Int(int) = &literal {
        int.base10_parse()
    } else {
        Err(syn::Error::new(literal.span(), "must be an integer"))
    };

    result.map_err(|err| TokenStream::from(err.into_compile_error()))
}

fn parse_bool(literal: &Lit) -> Result<bool, TokenStream> {
    let result = if let Lit::Bool(result) = &literal {
        Ok(result.value)
    } else {
        Err(syn::Error::new(literal.span(), "must be a boolean"))
    };

    result.map_err(|err| TokenStream::from(err.into_compile_error()))
}
