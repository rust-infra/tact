use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, Ident, ItemFn, Pat, PatIdent, PatType, ReturnType, Type, parse_macro_input,
};

#[proc_macro_attribute]
pub fn tool(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(input as ItemFn);

    match expand_tool(item_fn) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_tool(item_fn: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let fn_ident = &item_fn.sig.ident;
    let metadata_ident = format_ident!("{}_METADATA", fn_ident.to_string().to_uppercase());
    let wrapper_ident = format_ident!("{}Tool", to_pascal_case(&fn_ident.to_string()));
    let output = output_handling(&item_fn)?;

    if let Some((context, input_ty)) = extract_stateful_handler_input_type(&item_fn)? {
        match context {
            HandlerContext::Full => {
                expand_stateful_tool(item_fn, wrapper_ident, metadata_ident, input_ty, output)
            }
            HandlerContext::Workspace => {
                expand_workspace_tool(item_fn, wrapper_ident, metadata_ident, input_ty, output)
            }
            HandlerContext::Domain => {
                expand_domain_tool(item_fn, wrapper_ident, metadata_ident, input_ty, output)
            }
        }
    } else {
        expand_pure_tool(item_fn, wrapper_ident, metadata_ident, output)
    }
}

/// Which context a stateful handler asks for.
///
/// Two shapes exist because most tools need nothing from the context but the
/// workspace, and materialising a whole `ToolContext` to read one field is a
/// cost paid on every call. The macro picks the narrower impl from the
/// signature, so a tool cannot accidentally declare one and receive the other.
#[derive(Clone, Copy)]
enum HandlerContext {
    Full,
    Workspace,
    Domain,
}

struct OutputHandling {
    await_expr: proc_macro2::TokenStream,
}

fn output_handling(item_fn: &ItemFn) -> syn::Result<OutputHandling> {
    if item_fn.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            item_fn.sig.fn_token,
            "tool handlers must be async functions",
        ));
    }

    match &item_fn.sig.output {
        ReturnType::Default => Err(syn::Error::new_spanned(
            &item_fn.sig.ident,
            "tool handlers must return a value convertible to ToolCallResult",
        )),
        ReturnType::Type(_, ty) if is_result_type(ty) => Ok(OutputHandling {
            await_expr: quote! {
                let output = call_future.await?;
                Ok(crate::tool::IntoToolCallResult::into_tool_call_result(output))
            },
        }),
        ReturnType::Type(_, _) => Ok(OutputHandling {
            await_expr: quote! {
                let output = call_future.await;
                Ok(crate::tool::IntoToolCallResult::into_tool_call_result(output))
            },
        }),
    }
}

fn is_result_type(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };

    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };

    segment.ident == "Result"
}

/// A handler that takes `DomainHandles` instead of `ToolContext`.
///
/// The handler names the field it reads on `DomainHandles`, and the macro reads
/// that from the handler's own signature so the declaration cannot drift from the
/// use: `handles.skill_registry` means `DomainHandle::SkillRegistry`.
fn expand_domain_tool(
    item_fn: ItemFn,
    wrapper_ident: Ident,
    metadata_ident: Ident,
    input_ty: Type,
    output: OutputHandling,
) -> syn::Result<proc_macro2::TokenStream> {
    let fn_ident = &item_fn.sig.ident;
    let await_expr = output.await_expr;

    // Which handle does the body read? Derived from the parameter name the
    // handler used, so a handler that switches fields cannot forget to update a
    // separate declaration.
    let text = quote!(#item_fn).to_string();
    let mut needs = Vec::new();
    for (field, variant) in [
        (
            "skill_registry",
            quote!(crate::tool::DomainHandle::SkillRegistry),
        ),
        (
            "teammate_manager",
            quote!(crate::tool::DomainHandle::TeammateManager),
        ),
    ] {
        if text.contains(&format!(". {field}")) || text.contains(&format!(".{field}")) {
            needs.push(variant);
        }
    }
    if needs.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_fn.sig.ident,
            "a DomainHandles handler must read one of its fields (crate::tool::DomainHandles)",
        ));
    }

    Ok(quote! {
        #item_fn

        pub struct #wrapper_ident;

        #[async_trait::async_trait]
        impl crate::tool::Tool for #wrapper_ident {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                &#metadata_ident
            }

            fn input_schema(&self) -> serde_json::Value {
                crate::tool::input_schema::<#input_ty>()
            }

            fn domain_needs(&self) -> &'static [crate::tool::DomainHandle] {
                &[#(#needs),*]
            }

            async fn call(
                &self,
                context: crate::tool::ToolContext,
                input: serde_json::Value,
            ) -> anyhow::Result<crate::tool::ToolCallResult> {
                let handles = crate::tool::DomainHandles {
                    skill_registry: Some(context.skill_registry.clone()),
                    teammate_manager: Some(context.teammate_manager.clone()),
                };
                let input: #input_ty = serde_json::from_value(input)?;
                let call_future = #fn_ident(handles, input);
                #await_expr
            }

            fn call_in_domain<'a>(
                &'a self,
                handles: crate::tool::DomainHandles,
                input: serde_json::Value,
            ) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<crate::tool::ToolCallResult>> + Send + 'a>>> {
                Some(Box::pin(async move {
                    let input: #input_ty = serde_json::from_value(input)?;
                    let call_future = #fn_ident(handles, input);
                    #await_expr
                }))
            }
        }
    })
}

/// A handler that takes `Workspace` instead of `ToolContext`.
///
/// Generates `call_in_workspace` and leaves `call` to the trait's default (which
/// the adapter never reaches, because it asks the narrow method first). `call`
/// still has to *do* something if something calls it, so it is implemented as a
/// forwarded call using the workspace the context carries — the adapter never
/// takes that path, but a direct caller would otherwise get a silent no-op.
fn expand_workspace_tool(
    item_fn: ItemFn,
    wrapper_ident: Ident,
    metadata_ident: Ident,
    input_ty: Type,
    output: OutputHandling,
) -> syn::Result<proc_macro2::TokenStream> {
    let fn_ident = &item_fn.sig.ident;
    let await_expr = output.await_expr;

    Ok(quote! {
        #item_fn

        pub struct #wrapper_ident;

        #[async_trait::async_trait]
        impl crate::tool::Tool for #wrapper_ident {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                &#metadata_ident
            }

            fn input_schema(&self) -> serde_json::Value {
                crate::tool::input_schema::<#input_ty>()
            }

            async fn call(
                &self,
                context: crate::tool::ToolContext,
                input: serde_json::Value,
            ) -> anyhow::Result<crate::tool::ToolCallResult> {
                let workspace =
                    tact_kernel::tool::context::Workspace::new(context.work_dir.clone());
                let input: #input_ty = serde_json::from_value(input)?;
                let call_future = #fn_ident(workspace, input);
                #await_expr
            }

            fn call_in_workspace<'a>(
                &'a self,
                workspace: tact_kernel::tool::context::Workspace,
                input: serde_json::Value,
            ) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<crate::tool::ToolCallResult>> + Send + 'a>>> {
                Some(Box::pin(async move {
                    let input: #input_ty = serde_json::from_value(input)?;
                    let call_future = #fn_ident(workspace, input);
                    #await_expr
                }))
            }
        }
    })
}

fn expand_stateful_tool(
    item_fn: ItemFn,
    wrapper_ident: Ident,
    metadata_ident: Ident,
    input_ty: Type,
    output: OutputHandling,
) -> syn::Result<proc_macro2::TokenStream> {
    let fn_ident = &item_fn.sig.ident;
    let await_expr = output.await_expr;

    Ok(quote! {
        #item_fn

        pub struct #wrapper_ident;

        #[async_trait::async_trait]
        impl crate::tool::Tool for #wrapper_ident {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                &#metadata_ident
            }

            fn input_schema(&self) -> serde_json::Value {
                crate::tool::input_schema::<#input_ty>()
            }

            async fn call(
                &self,
                context: crate::tool::ToolContext,
                input: serde_json::Value,
            ) -> anyhow::Result<crate::tool::ToolCallResult> {
                let input: #input_ty = serde_json::from_value(input)?;
                let call_future = #fn_ident(context, input);
                #await_expr
            }
        }
    })
}

fn expand_pure_tool(
    mut item_fn: ItemFn,
    wrapper_ident: Ident,
    metadata_ident: Ident,
    output: OutputHandling,
) -> syn::Result<proc_macro2::TokenStream> {
    let fn_ident = item_fn.sig.ident.clone();
    let input_ident = format_ident!("{}Input", to_pascal_case(&fn_ident.to_string()));
    let await_expr = output.await_expr;

    let args = item_fn
        .sig
        .inputs
        .iter()
        .map(extract_plain_arg)
        .collect::<syn::Result<Vec<_>>>()?;
    clear_parameter_attrs(&mut item_fn);
    let fields = args.iter().map(|arg| {
        let attrs = &arg.attrs;
        let ident = &arg.ident;
        let ty = &arg.ty;
        quote! {
            #(#attrs)*
            pub #ident: #ty
        }
    });
    let call_args = args.iter().map(|arg| {
        let ident = &arg.ident;
        quote! { input.#ident }
    });

    Ok(quote! {
        #item_fn

        #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
        pub struct #input_ident {
            #(#fields,)*
        }

        pub struct #wrapper_ident;

        #[async_trait::async_trait]
        impl crate::tool::Tool for #wrapper_ident {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                &#metadata_ident
            }

            fn input_schema(&self) -> serde_json::Value {
                crate::tool::input_schema::<#input_ident>()
            }

            async fn call(
                &self,
                _context: crate::tool::ToolContext,
                input: serde_json::Value,
            ) -> anyhow::Result<crate::tool::ToolCallResult> {
                let input: #input_ident = serde_json::from_value(input)?;
                let call_future = #fn_ident(#(#call_args),*);
                #await_expr
            }
        }
    })
}

struct PlainArg {
    attrs: Vec<Attribute>,
    ident: Ident,
    ty: Type,
}

fn extract_plain_arg(arg: &FnArg) -> syn::Result<PlainArg> {
    let FnArg::Typed(PatType { attrs, pat, ty, .. }) = arg else {
        return Err(syn::Error::new_spanned(
            arg,
            "tool handlers must not take self",
        ));
    };

    let Pat::Ident(PatIdent { ident, .. }) = pat.as_ref() else {
        return Err(syn::Error::new_spanned(
            pat,
            "plain tool handler arguments must be named like `a: i32`",
        ));
    };

    Ok(PlainArg {
        attrs: attrs.clone(),
        ident: ident.clone(),
        ty: ty.as_ref().clone(),
    })
}

fn extract_stateful_handler_input_type(
    item_fn: &ItemFn,
) -> syn::Result<Option<(HandlerContext, Type)>> {
    let mut inputs = item_fn.sig.inputs.iter();
    let Some(context_arg) = inputs.next() else {
        return Ok(None);
    };
    let Some(input_arg) = inputs.next() else {
        return Ok(None);
    };

    if inputs.next().is_some() {
        return Ok(None);
    }

    let context = if is_type_named(context_arg, "ToolContext") {
        HandlerContext::Full
    } else if is_type_named(context_arg, "Workspace") {
        HandlerContext::Workspace
    } else if is_type_named(context_arg, "DomainHandles") {
        HandlerContext::Domain
    } else {
        return Ok(None);
    };
    let input_ty = extract_arg_type(input_arg)?;

    Ok(Some((context, input_ty)))
}

fn clear_parameter_attrs(item_fn: &mut ItemFn) {
    for arg in &mut item_fn.sig.inputs {
        if let FnArg::Typed(PatType { attrs, .. }) = arg {
            attrs.clear();
        }
    }
}

fn is_type_named(arg: &FnArg, expected: &str) -> bool {
    let Ok(ty) = extract_arg_type(arg) else {
        return false;
    };

    let Type::Path(type_path) = &ty else {
        return false;
    };

    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };

    segment.ident == expected
}

fn extract_arg_type(arg: &FnArg) -> syn::Result<Type> {
    let FnArg::Typed(PatType { ty, .. }) = arg else {
        return Err(syn::Error::new_spanned(
            arg,
            "tool handlers must not take self",
        ));
    };

    Ok(ty.as_ref().clone())
}

fn to_pascal_case(value: &str) -> String {
    let mut output = String::new();
    let mut uppercase_next = true;

    for ch in value.chars() {
        if ch == '_' {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            output.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(ch);
        }
    }

    output
}
