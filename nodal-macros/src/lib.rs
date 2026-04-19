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
    let service_name_template = service_name.expect("service attribute requires 'name' parameter");
    let service_version = service_version.expect("service attribute requires 'version' parameter");

    // Extract template parameters from service name (e.g., "weather.{id}" -> ["id"])
    let service_template_params = extract_template_params(&service_name_template);

    // Generate parameter identifiers for the service function signature
    let param_idents: Vec<syn::Ident> = service_template_params
        .iter()
        .map(|p| syn::Ident::new(p, proc_macro2::Span::call_site()))
        .collect();

    // Build the actual service name (without templates, just the plain name)
    // e.g., "weather.{id}" -> "weather"
    let service_name = service_name_template
        .split('.')
        .next()
        .unwrap_or(&service_name_template);

    let trait_name = &trait_item.ident;
    let ext_trait_name = syn::Ident::new(&format!("{}Ext", trait_name), trait_name.span());

    // add trait bounds for the associated Context type
    let where_clause = trait_item.generics.make_where_clause();
    where_clause
        .predicates
        .push(syn::parse_quote!(Self::Context: ::nodal::ServiceContext));

    let mut endpoint_methods = Vec::new();
    let mut stream_methods = Vec::new();

    for item in &mut trait_item.items {
        if let TraitItem::Fn(method) = item {
            // check if this method has a #[stream] attribute
            let mut stream_name = None;
            let mut stream_subject = None;
            let mut stream_storage = None;
            let mut stream_message = None;

            // check if this method has an #[endpoint] attribute
            let mut endpoint_subject = None;
            method.attrs.retain(|attr| {
                if attr.path().is_ident("stream") {
                    let _ = attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident("name") {
                            let value = meta.value()?;
                            let s: syn::LitStr = value.parse()?;
                            stream_name = Some(s.value());
                            Ok(())
                        } else if meta.path.is_ident("subject") {
                            let value = meta.value()?;
                            let s: syn::LitStr = value.parse()?;
                            stream_subject = Some(s.value());
                            Ok(())
                        } else if meta.path.is_ident("storage") {
                            let value = meta.value()?;
                            let path: syn::Path = value.parse()?;
                            stream_storage = Some(path);
                            Ok(())
                        } else if meta.path.is_ident("message") {
                            let value = meta.value()?;
                            let ty: syn::Type = value.parse()?;
                            stream_message = Some(ty);
                            Ok(())
                        } else {
                            Err(meta.error("unsupported stream attribute"))
                        }
                    });
                    false // remove the attribute
                } else if attr.path().is_ident("endpoint") {
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

            if let (Some(name), Some(subject)) = (stream_name, stream_subject) {
                let method_name = method.sig.ident.clone();

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

                stream_methods.push((method_name, name, subject, stream_storage, stream_message));
            }

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

    // generate stream handler structs and registrations
    let mut stream_handler_structs = Vec::new();
    let mut stream_handler_debug_impls = Vec::new();
    let mut stream_handler_impls = Vec::new();

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
                        rqctx: ::nodal::endpoint::RequestContext<T::Context>,
                        body: ::nodal::Bytes,
                    ) -> Result<::nodal::Bytes, ::nodal::BoxError> {
                        let request: ::nodal::endpoint::Request<_> = ::serde_json::from_slice(&body)?;
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
                        rqctx: ::nodal::endpoint::RequestContext<T::Context>,
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

        // Build the subject expression by applying service template parameters
        let subject_expr = build_subject_expr(subject, &service_template_params);

        endpoint_registrations.push(quote! {
            endpoints.push(::nodal::Endpoint {
                subject: #subject_expr,
                handler: ::std::sync::Arc::new(#handler_name::<Self>(::std::marker::PhantomData)),
                request_schema: ::schemars::schema_for!(#request_type),
                response_schema: ::schemars::schema_for!(#response_type),
            });
        });
    }

    // Build the service function signature with optional parameters
    let service_fn_signature = if service_template_params.is_empty() {
        quote! {
            fn service(context: Self::Context) -> ::nodal::Service<Self::Context>
        }
    } else {
        quote! {
            fn service(context: Self::Context, #(#param_idents: impl ::std::fmt::Display),*) -> ::nodal::Service<Self::Context>
        }
    };

    // generate stream handlers and registrations
    for (method_name, _stream_name, _stream_subject, _storage_type, _message_type) in
        &stream_methods
    {
        let handler_name = syn::Ident::new(
            &format!("{}StreamHandler", snake_to_pascal(&method_name.to_string())),
            method_name.span(),
        );

        stream_handler_structs.push(quote! {
            struct #handler_name<T>(::std::marker::PhantomData<T>);
        });

        stream_handler_debug_impls.push(quote! {
            impl<T> ::std::fmt::Debug for #handler_name<T> {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    f.debug_struct(stringify!(#handler_name)).finish()
                }
            }
        });

        stream_handler_impls.push(quote! {
            #[::nodal::async_trait::async_trait]
            impl<T> ::nodal::stream::StreamHandler<T::Context> for #handler_name<T>
            where
                T: #trait_name + Send + Sync + 'static,
                T::Context: ::nodal::ServiceContext,
            {
                async fn handle_stream(
                    &self,
                    ctx: ::nodal::stream::StreamContext<T::Context>,
                ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                    T::#method_name(ctx).await?;
                    Ok(())
                }
            }
        });
    }

    // generate stream registrations
    let mut stream_registrations = Vec::new();
    for (method_name, stream_name, stream_subject, storage_type, message_type) in &stream_methods {
        let handler_name = syn::Ident::new(
            &format!("{}StreamHandler", snake_to_pascal(&method_name.to_string())),
            method_name.span(),
        );

        let storage_expr = if let Some(storage) = storage_type {
            quote! { #storage }
        } else {
            quote! { ::async_nats::jetstream::stream::StorageType::File }
        };

        let message_schema = if let Some(msg_type) = message_type {
            quote! { ::schemars::schema_for!(#msg_type) }
        } else {
            quote! { ::schemars::schema_for!(()) }
        };

        let subject_prefix_expr = build_subject_prefix_expr(&service_template_params);

        stream_registrations.push(quote! {
            streams.push(::nodal::Stream {
                subject_prefix: format!("{}.{}", #service_name, #subject_prefix_expr),
                config: ::async_nats::jetstream::stream::Config {
                    name: format!("{}_{}", #service_name.to_string().to_uppercase(), #stream_name.to_string()),
                    subjects: vec![format!("{}.{}.{}", #service_name, #subject_prefix_expr, #stream_subject)],
                    storage: #storage_expr,
                    ..Default::default()
                },
                handler: ::std::sync::Arc::new(#handler_name::<Self>(::std::marker::PhantomData)),
                message_schema: #message_schema,
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

        // generate stream handler structs
        #(#stream_handler_structs)*

        // generate Debug implementations for stream handlers
        #(#stream_handler_debug_impls)*

        // generate stream handler implementations
        #(#stream_handler_impls)*

        // extension trait for the service() method with default implementation
        pub trait #ext_trait_name: #trait_name + Sized
        where
            Self: Send + Sync + 'static,
            Self::Context: ::nodal::ServiceContext,
        {
            #service_fn_signature;
        }

        // blanket implementation of the extension trait
        impl<T> #ext_trait_name for T
        where
            T: #trait_name + Send + Sync + 'static,
            T::Context: ::nodal::ServiceContext,
        {
            #service_fn_signature {
                let mut endpoints = Vec::new();
                let mut streams = Vec::new();

                #(#endpoint_registrations)*

                #(#stream_registrations)*

                ::nodal::Service {
                    name: #service_name.to_string(),
                    version: #service_version.to_string(),
                    endpoints,
                    streams,
                    context,
                }
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn endpoint(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is handled by the service macro
    input
}

#[proc_macro_attribute]
pub fn stream(_args: TokenStream, input: TokenStream) -> TokenStream {
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

// Extract template parameters from a template string
// e.g., "weather.{id}.{zone}" -> ["id", "zone"]
fn extract_template_params(template: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut param = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                param.push(ch);
            }
            if !param.is_empty() {
                params.push(param);
            }
        }
    }

    params
}

// Build an expression that constructs the subject by replacing template parameters
// with the corresponding parameters from the service name
// e.g., subject "wind_speed.{id}" with service params ["id"] -> format!("wind_speed.{}", id)
fn build_subject_expr(subject: &str, service_params: &[String]) -> proc_macro2::TokenStream {
    if service_params.is_empty() {
        // No parameters, just return the subject as a string
        return quote! { #subject.to_string() };
    }

    // Replace {param} with {} for format! macro
    let mut format_str = String::new();
    for _ in service_params {
        format_str.push_str("{}.");
    }
    format_str.push_str(subject);

    // Generate parameter identifiers in the order they appear in this subject
    let param_idents: Vec<proc_macro2::TokenStream> = service_params
        .iter()
        .map(|p| {
            let ident = syn::Ident::new(p, proc_macro2::Span::call_site());
            quote! { #ident }
        })
        .collect();

    quote! {
        format!(#format_str, #(#param_idents),*)
    }
}

fn build_subject_prefix_expr(service_params: &[String]) -> proc_macro2::TokenStream {
    let format_str = service_params
        .iter()
        .map(|_| "{}")
        .collect::<Vec<_>>()
        .join(".");

    // Generate parameter identifiers in the order they appear in this subject
    let param_idents: Vec<proc_macro2::TokenStream> = service_params
        .iter()
        .map(|p| {
            let ident = syn::Ident::new(p, proc_macro2::Span::call_site());
            quote! { #ident }
        })
        .collect();

    quote! {
        format!(#format_str, #(#param_idents),*)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

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

    #[test]
    fn test_extract_template_params_none() {
        assert_eq!(extract_template_params("wind_speed"), Vec::<String>::new());
    }

    #[test]
    fn test_extract_template_params_single() {
        assert_eq!(extract_template_params("weather.{id}"), vec!["id"]);
    }

    #[test]
    fn test_extract_template_params_multiple() {
        assert_eq!(
            extract_template_params("weather.{id}.{zone}"),
            vec!["id", "zone"]
        );
    }

    #[test]
    fn test_extract_template_params_empty_braces() {
        assert_eq!(extract_template_params("weather.{}"), Vec::<String>::new());
    }

    #[test]
    fn test_extract_template_params_mixed() {
        assert_eq!(
            extract_template_params("prefix.{param1}.middle.{param2}.suffix"),
            vec!["param1", "param2"]
        );
    }

    #[test]
    fn test_build_subject_expr_no_params() {
        let subject = "wind_speed";
        let service_params: Vec<String> = vec![];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! { "wind_speed".to_string() };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_build_subject_expr_single_param() {
        let subject = "sensor_data";
        let service_params = vec!["id".to_string()];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! {
            format!("{}.sensor_data", id)
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_build_subject_expr_multiple_params() {
        let subject = "wind_speed";
        let service_params = vec!["region".to_string(), "id".to_string()];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! {
            format!("{}.{}.wind_speed", region, id)
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_build_subject_expr_three_params() {
        let subject = "data";
        let service_params = vec![
            "namespace".to_string(),
            "service".to_string(),
            "id".to_string(),
        ];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! {
            format!("{}.{}.{}.data", namespace, service, id)
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_build_subject_expr_subject_with_special_chars() {
        let subject = "sensor.temperature_reading";
        let service_params = vec!["id".to_string()];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! {
            format!("{}.sensor.temperature_reading", id)
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_build_subject_expr_empty_subject() {
        let subject = "";
        let service_params = vec!["id".to_string()];

        let result = build_subject_expr(subject, &service_params);
        let expected = quote! {
            format!("{}.", id)
        };

        assert_eq!(result.to_string(), expected.to_string());
    }
}
