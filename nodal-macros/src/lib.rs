use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemTrait, ReturnType, TraitItem, parse_macro_input};

#[proc_macro_attribute]
pub fn service(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut trait_item = parse_macro_input!(input as ItemTrait);

    let mut service_name = None;
    let mut service_version = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            service_name = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("version") {
            service_version = Some(meta.value()?.parse::<syn::LitStr>()?.value());
            Ok(())
        } else {
            Err(meta.error("unsupported service attribute"))
        }
    });
    parse_macro_input!(args with parser);
    let service_name = service_name.expect("service attribute requires 'name' parameter");
    let service_version = service_version.expect("service attribute requires 'version' parameter");

    let trait_name = &trait_item.ident;
    let ext_trait_name = syn::Ident::new(&format!("{}Ext", trait_name), trait_name.span());

    // add trait bounds for the associated Context type
    let where_clause = trait_item.generics.make_where_clause();
    where_clause
        .predicates
        .push(syn::parse_quote!(Self::Context: ::nodal::ServiceContext));

    let mut endpoint_methods = Vec::new();
    for item in &mut trait_item.items {
        if let TraitItem::Fn(method) = item {
            // check if this method has an #[endpoint] attribute
            let mut endpoint_subject = None;
            method.attrs.retain(|attr| {
                if attr.path().is_ident("endpoint") {
                    let _ = attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident("subject") {
                            let value = meta.value()?;
                            let s: syn::LitStr = value.parse()?;
                            endpoint_subject = Some(s.value());
                            Ok(())
                        } else {
                            Err(meta.error("unsupported endpoint attribute"))
                        }
                    });
                    false // remove the attribute
                } else {
                    true // keep other attributes
                }
            });

            if let Some(subject) = endpoint_subject {
                let method_name = method.sig.ident.clone();
                let has_body_param = method.sig.inputs.len() > 1;

                // extract request type if present (from Request<T>)
                let request_type = if has_body_param
                    && let syn::FnArg::Typed(arg) = &method.sig.inputs[1]
                    && let syn::Type::Path(type_path) = &*arg.ty
                    && let Some(segment) = type_path.path.segments.last()
                    && segment.ident == "Request"
                    && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
                    && let Some(syn::GenericArgument::Type(ty)) = args.args.first()
                {
                    ty.clone()
                } else {
                    syn::parse_str("()").unwrap()
                };

                // extract response type from Result<Response<T>, Error>
                let response_type = if let ReturnType::Type(_, ref ty) = method.sig.output {
                    extract_response_type(ty).unwrap_or(syn::parse_str("()").unwrap())
                } else {
                    syn::parse_str("()").unwrap()
                };

                // add `Send` bound to the return type if it's async
                if method.sig.asyncness.is_some()
                    && let ReturnType::Type(_, ref mut ty) = method.sig.output
                {
                    // wrap the return type with + Send
                    let original_ty = (**ty).clone();
                    **ty = syn::parse_quote!(
                        impl ::std::future::Future<Output = #original_ty> + Send
                    );
                    // remove async keyword since we're using impl Future now
                    method.sig.asyncness = None;
                }

                endpoint_methods.push((
                    method_name,
                    subject,
                    has_body_param,
                    request_type,
                    response_type,
                ));
            }
        }
    }

    // generate endpoint handler structs and registrations
    let mut handler_structs = Vec::new();
    let mut handler_debug_impls = Vec::new();
    let mut handler_impls = Vec::new();
    let mut endpoint_registrations = Vec::new();

    for (method_name, subject, has_body_param, request_type, response_type) in &endpoint_methods {
        // convert snake_case to PascalCase for handler name
        let handler_name = syn::Ident::new(
            &format!("{}Handler", snake_to_pascal(&method_name.to_string())),
            method_name.span(),
        );

        // handler struct definition - generic over T
        handler_structs.push(quote! {
            struct #handler_name<T>(::std::marker::PhantomData<T>);
        });

        // manual Debug implementation
        handler_debug_impls.push(quote! {
            impl<T> ::std::fmt::Debug for #handler_name<T> {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    f.debug_struct(stringify!(#handler_name)).finish()
                }
            }
        });

        // handler implementation - with proper trait bounds
        let handler_impl = if *has_body_param {
            quote! {
                #[::nodal::async_trait::async_trait]
                impl<T> ::nodal::EndpointHandler<T::Context> for #handler_name<T>
                where
                    T: #trait_name + Send + Sync + 'static,
                    T::Context: ::nodal::ServiceContext,
                {
                    async fn handle_request(
                        &self,
                        rqctx: ::nodal::RequestContext<T::Context>,
                        body: ::nodal::Bytes,
                    ) -> Result<::nodal::Bytes, ::nodal::BoxError> {
                        let request: ::nodal::Request<_> = ::serde_json::from_slice(&body)?;
                        let result = T::#method_name(rqctx, request).await;

                        match result {
                            Ok(response) => {
                                let json = ::serde_json::to_vec(&response)?;
                                Ok(::nodal::Bytes::from(json))
                            }
                            Err(e) => Err(Box::new(e) as ::nodal::BoxError),
                        }
                    }
                }
            }
        } else {
            quote! {
                #[::nodal::async_trait::async_trait]
                impl<T> ::nodal::EndpointHandler<T::Context> for #handler_name<T>
                where
                    T: #trait_name + Send + Sync + 'static,
                    T::Context: ::nodal::ServiceContext,
                {
                    async fn handle_request(
                        &self,
                        rqctx: ::nodal::RequestContext<T::Context>,
                        _body: ::nodal::Bytes,
                    ) -> Result<::nodal::Bytes, ::nodal::BoxError> {
                        let result = T::#method_name(rqctx).await;

                        match result {
                            Ok(response) => {
                                let json = ::serde_json::to_vec(&response)?;
                                Ok(::nodal::Bytes::from(json))
                            }
                            Err(e) => Err(Box::new(e) as ::nodal::BoxError),
                        }
                    }
                }
            }
        };

        handler_impls.push(handler_impl);

        endpoint_registrations.push(quote! {
            endpoints.push(::nodal::Endpoint {
                subject: #subject.to_string(),
                handler: ::std::sync::Arc::new(#handler_name::<Self>(::std::marker::PhantomData)),
                request_schema: ::schemars::schema_for!(#request_type),
                response_schema: ::schemars::schema_for!(#response_type),
            });
        });
    }

    let expanded = quote! {
        #trait_item

        // generate handler structs outside the impl block
        #(#handler_structs)*

        // generate Debug implementations for handlers
        #(#handler_debug_impls)*

        // generate handler implementations
        #(#handler_impls)*

        // extension trait for the service() method with default implementation
        pub trait #ext_trait_name: #trait_name + Sized
        where
            Self: Send + Sync + 'static,
            Self::Context: ::nodal::ServiceContext,
        {
            fn service(context: Self::Context) -> ::nodal::Service<Self::Context> {
                let mut endpoints = Vec::new();

                #(#endpoint_registrations)*

                ::nodal::Service {
                    name: #service_name.to_string(),
                    version: #service_version.to_string(),
                    endpoints,
                    context,
                }
            }
        }

        // blanket implementation of the extension trait
        impl<T> #ext_trait_name for T
        where
            T: #trait_name + Send + Sync + 'static,
            T::Context: ::nodal::ServiceContext,
        {}
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn endpoint(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is handled by the service macro
    input
}

// helper function to extract the response type T from Result<Response<T>, Error>
fn extract_response_type(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        // look for Result<Response<T>, Error>
        if let Some(segment) = type_path.path.segments.last()
            && segment.ident == "Result"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        {
            // get the first type argument (Response<T>)
            if let Some(syn::GenericArgument::Type(syn::Type::Path(response_path))) =
                args.args.first()
                && let Some(response_segment) = response_path.path.segments.last()
                && response_segment.ident == "Response"
                && let syn::PathArguments::AngleBracketed(response_args) =
                    &response_segment.arguments
            {
                // get T from Response<T>
                if let Some(syn::GenericArgument::Type(inner_ty)) = response_args.args.first() {
                    return Some(inner_ty.clone());
                }
            }
        }
    }

    None
}

fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut c = word.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_to_pascal_simple() {
        assert_eq!(snake_to_pascal("hello_world"), "HelloWorld");
    }

    #[test]
    fn test_snake_to_pascal_single_word() {
        assert_eq!(snake_to_pascal("hello"), "Hello");
    }

    #[test]
    fn test_snake_to_pascal_empty_string() {
        assert_eq!(snake_to_pascal(""), "");
    }

    #[test]
    fn test_snake_to_pascal_multiple_underscores() {
        assert_eq!(snake_to_pascal("hello__world"), "HelloWorld");
    }

    #[test]
    fn test_snake_to_pascal_leading_underscore() {
        assert_eq!(snake_to_pascal("_hello_world"), "HelloWorld");
    }

    #[test]
    fn test_snake_to_pascal_trailing_underscore() {
        assert_eq!(snake_to_pascal("hello_world_"), "HelloWorld");
    }

    #[test]
    fn test_snake_to_pascal_many_words() {
        assert_eq!(snake_to_pascal("this_is_a_long_name"), "ThisIsALongName");
    }

    #[test]
    fn test_snake_to_pascal_single_char_words() {
        assert_eq!(snake_to_pascal("a_b_c"), "ABC");
    }

    #[test]
    fn test_snake_to_pascal_already_capitalized() {
        assert_eq!(snake_to_pascal("Hello_World"), "HelloWorld");
    }
}
