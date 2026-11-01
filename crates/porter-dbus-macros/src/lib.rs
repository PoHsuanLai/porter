//! `#[bus_interface]`: a bus interface is written once, as the trait its clients call, and this
//! attribute makes the rest of what the interface needs.
//!
//! ```ignore
//! #[bus_interface(
//!     interface = "org.quire.Spaces1",
//!     default_service = "org.quire.Accounts1",
//!     default_path = "/org/quire/Spaces1"
//! )]
//! pub trait Spaces {
//!     /// Every desktop-wide Space.
//!     fn list(&self) -> zbus::Result<Vec<(String, Details)>>;
//!     #[zbus(signal)]
//!     fn changed(&self, id: &str, what: &str) -> zbus::Result<()>;
//! }
//! ```
//!
//! The trait stays what it was, a `#[zbus::proxy]` trait (`SpacesProxy`), and the attribute adds:
//! - `SpacesServer`, the trait a daemon implements. Each method takes the message's `Call` first
//!   and then the arguments by value (`&str` is a `String`, `&[T]` a `Vec<T>`, any other `&T` a
//!   `T`); it answers `Result<T, Self::Error>`. A property takes the caller's header, absent when
//!   zbus asks for the value to announce a change. A signal has no method: the daemon emits it
//!   through the served object.
//! - `SpacesServed<S>`, the zbus object that serves an `S: SpacesServer`. Its methods and
//!   signals are the trait's, so the introspection is the trait's: there is no second declaration
//!   to keep in step.
//! - an implementation of `SpacesServer` for `Unserved`, the type no object is ever registered
//!   over, so that `SpacesServed::<Unserved>::introspect` can write the interface's element
//!   without a daemon.
//!
//! Two attributes of the trait's own: `#[bus(docs)]` on a member puts its doc comment into the
//! introspection (a member without it has none there), and `#[bus(out_args("a", "b"))]` names
//! the parts of a tuple answer.
//!
//! The expansion names `::porter_dbus::Call` and `::porter_dbus::Unserved`, so a crate that uses it
//! depends on porter-dbus, which declares its own interfaces under `extern crate self as porter_dbus`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Error, Expr, ExprLit, FnArg, GenericArgument, Ident, ItemTrait, Lit, Meta, Pat,
    PathArguments, ReturnType, Token, TraitItem, TraitItemFn, Type, Visibility, parse_macro_input,
    parse_quote,
};

/// See the crate documentation.
#[proc_macro_attribute]
pub fn bus_interface(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    let item = parse_macro_input!(item as ItemTrait);
    match expand(&args, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// What a member of the trait is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Method,
    Property,
    Signal,
}

/// One member of the trait.
struct Member {
    name: Ident,
    kind: Kind,
    /// The doc comments, copied to the server trait; and to the served object only with `docs`.
    docs: Vec<Attribute>,
    introspect_docs: bool,
    out_args: Option<TokenStream2>,
    /// The arguments after `&self`, as the proxy writes them.
    args: Vec<(Ident, Type)>,
    /// What the proxy's `zbus::Result<T>` holds.
    ok: Type,
}

fn expand(args: &TokenStream2, mut item: ItemTrait) -> syn::Result<TokenStream2> {
    let interface = interface_name(args)?;
    let proxy = item.ident.clone();
    let server = format_ident!("{proxy}Server");
    let served = format_ident!("{proxy}Served");
    let vis = item.vis.clone();

    let mut members = Vec::new();
    for entry in &mut item.items {
        match entry {
            TraitItem::Fn(function) => members.push(Member::parse(function)?),
            other => {
                return Err(Error::new_spanned(
                    other,
                    "a bus interface holds methods, properties and signals only",
                ));
            }
        }
    }

    let server_methods = members.iter().filter_map(Member::server_method);
    let served_members = members.iter().map(|m| m.served_member(&server, &vis));
    let unserved_methods = members.iter().filter_map(Member::unserved_method);
    let server_doc = format!(
        "The daemon's side of `{interface}`: what the object that answers implements, and \
         registers as [`{served}`]. Each method is given the [`Call`](::porter_dbus::Call) of the message \
         first."
    );
    let served_doc = format!(
        "The zbus object that serves `{interface}` from an `S` that implements [`{server}`]. \
         Register it with `ObjectServer::at`; its signals are emitted through it."
    );

    Ok(quote! {
        #[::zbus::proxy(#args)]
        #item

        #[doc = #server_doc]
        #vis trait #server: ::core::marker::Send + ::core::marker::Sync + 'static {
            /// What a method answers instead of its value.
            type Error: ::zbus::DBusError + ::core::marker::Send;
            #(#server_methods)*
        }

        #[doc = #served_doc]
        #[derive(Debug)]
        #vis struct #served<S>(pub S);

        #[::zbus::interface(name = #interface)]
        impl<S: #server> #served<S> {
            #(#served_members)*
        }

        impl #served<::porter_dbus::Unserved> {
            /// Writes this interface's introspection into `xml`, as an `<interface>` element one
            /// level in: a document collects its interfaces' own.
            #vis fn introspect(xml: &mut ::std::string::String) {
                ::zbus::object_server::Interface::introspect_to_writer(
                    &Self(::porter_dbus::Unserved),
                    xml,
                    1,
                );
            }
        }

        impl #server for ::porter_dbus::Unserved {
            type Error = ::zbus::fdo::Error;
            #(#unserved_methods)*
        }
    })
}

/// The `interface = "..."` of the attribute's arguments.
fn interface_name(args: &TokenStream2) -> syn::Result<String> {
    let metas = syn::parse::Parser::parse2(
        Punctuated::<Meta, Token![,]>::parse_terminated,
        args.clone(),
    )?;
    for meta in &metas {
        if let Meta::NameValue(pair) = meta
            && pair.path.is_ident("interface")
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(name),
                ..
            }) = &pair.value
        {
            return Ok(name.value());
        }
    }
    Err(Error::new_spanned(
        args,
        "bus_interface needs interface = \"...\"",
    ))
}

impl Member {
    fn parse(function: &mut TraitItemFn) -> syn::Result<Self> {
        let mut kind = Kind::Method;
        let mut introspect_docs = false;
        let mut out_args = None;
        for attribute in &function.attrs {
            if attribute.path().is_ident("zbus") {
                attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("signal") {
                        kind = Kind::Signal;
                    } else if meta.path.is_ident("property") {
                        kind = Kind::Property;
                    }
                    if meta.input.peek(Token![=]) {
                        let _: Expr = meta.value()?.parse()?;
                    } else if meta.input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        let _: TokenStream2 = content.parse()?;
                    }
                    Ok(())
                })?;
            } else if attribute.path().is_ident("bus") {
                attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("docs") {
                        introspect_docs = true;
                    } else if meta.path.is_ident("out_args") {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        out_args = Some(content.parse::<TokenStream2>()?);
                    } else {
                        return Err(meta.error("expected `docs` or `out_args(...)`"));
                    }
                    Ok(())
                })?;
            }
        }
        let docs = function
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("doc"))
            .cloned()
            .collect();
        function.attrs.retain(|a| !a.path().is_ident("bus"));

        let mut inputs = function.sig.inputs.iter();
        match inputs.next() {
            Some(FnArg::Receiver(receiver)) if receiver.reference.is_some() => {}
            _ => return Err(Error::new_spanned(&function.sig, "expected `&self`")),
        }
        let mut args = Vec::new();
        for input in inputs {
            let FnArg::Typed(typed) = input else {
                return Err(Error::new_spanned(input, "unexpected receiver"));
            };
            let Pat::Ident(name) = &*typed.pat else {
                return Err(Error::new_spanned(
                    &typed.pat,
                    "an argument has a plain name",
                ));
            };
            args.push((name.ident.clone(), (*typed.ty).clone()));
        }
        if kind == Kind::Property && !args.is_empty() {
            return Err(Error::new_spanned(
                &function.sig,
                "a property is read here, with no arguments",
            ));
        }
        Ok(Self {
            name: function.sig.ident.clone(),
            kind,
            docs,
            introspect_docs,
            out_args,
            args,
            ok: ok_type(&function.sig.output)?,
        })
    }

    /// The argument types the server receives, and the names, in order.
    fn owned_args(&self) -> (Vec<&Ident>, Vec<Type>) {
        let names = self.args.iter().map(|(name, _)| name).collect();
        let types = self.args.iter().map(|(_, ty)| owned(ty)).collect();
        (names, types)
    }

    /// The method of the server trait. A signal has none.
    fn server_method(&self) -> Option<TokenStream2> {
        let name = &self.name;
        let docs = &self.docs;
        let ok = &self.ok;
        let (names, types) = self.owned_args();
        Some(match self.kind {
            Kind::Method => quote! {
                #(#docs)*
                fn #name(&self, call: ::porter_dbus::Call<'_> #(, #names: #types)*)
                    -> impl ::core::future::Future<
                        Output = ::core::result::Result<#ok, Self::Error>
                    > + ::core::marker::Send;
            },
            Kind::Property => quote! {
                #(#docs)*
                fn #name(
                    &self,
                    header: ::core::option::Option<::zbus::message::Header<'_>>,
                ) -> impl ::core::future::Future<Output = ::zbus::fdo::Result<#ok>>
                    + ::core::marker::Send;
            },
            Kind::Signal => return None,
        })
    }

    /// The member of the served object.
    fn served_member(&self, server: &Ident, vis: &Visibility) -> TokenStream2 {
        let name = &self.name;
        let ok = &self.ok;
        let docs = if self.introspect_docs {
            self.docs.clone()
        } else {
            Vec::new()
        };
        let (names, types) = self.owned_args();
        match self.kind {
            Kind::Method => {
                let out_args = self
                    .out_args
                    .as_ref()
                    .map(|out| quote!(#[zbus(out_args(#out))]));
                quote! {
                    #(#docs)*
                    #out_args
                    // The three extractors are always taken, and with the arguments they make
                    // more than clippy likes; the arguments are the interface's.
                    #[allow(clippy::too_many_arguments)]
                    async fn #name(
                        &self,
                        #[zbus(header)] header: ::zbus::message::Header<'_>,
                        #[zbus(signal_emitter)] emitter: ::zbus::object_server::SignalEmitter<'_>,
                        #[zbus(connection)] connection: &::zbus::Connection,
                        #(#names: #types,)*
                    ) -> ::core::result::Result<#ok, <S as #server>::Error> {
                        self.0
                            .#name(::porter_dbus::Call { header, emitter, connection } #(, #names)*)
                            .await
                    }
                }
            }
            Kind::Property => quote! {
                #(#docs)*
                #[zbus(property)]
                async fn #name(
                    &self,
                    #[zbus(header)] header: ::core::option::Option<::zbus::message::Header<'_>>,
                ) -> ::zbus::fdo::Result<#ok> {
                    self.0.#name(header).await
                }
            },
            Kind::Signal => {
                let args = self.args.iter().map(|(name, ty)| quote!(#name: #ty));
                quote! {
                    #(#docs)*
                    #[zbus(signal)]
                    #vis async fn #name(
                        emitter: &::zbus::object_server::SignalEmitter<'_>
                        #(, #args)*
                    ) -> ::zbus::Result<()>;
                }
            }
        }
    }

    /// The method of `Unserved`'s implementation: it never answers, for nothing calls it.
    fn unserved_method(&self) -> Option<TokenStream2> {
        let name = &self.name;
        let ok = &self.ok;
        let (_, types) = self.owned_args();
        Some(match self.kind {
            Kind::Method => quote! {
                fn #name(&self, _: ::porter_dbus::Call<'_> #(, _: #types)*)
                    -> impl ::core::future::Future<
                        Output = ::core::result::Result<#ok, Self::Error>
                    > + ::core::marker::Send
                {
                    ::core::future::pending()
                }
            },
            Kind::Property => quote! {
                fn #name(
                    &self,
                    _: ::core::option::Option<::zbus::message::Header<'_>>,
                ) -> impl ::core::future::Future<Output = ::zbus::fdo::Result<#ok>>
                    + ::core::marker::Send
                {
                    ::core::future::pending()
                }
            },
            Kind::Signal => return None,
        })
    }
}

/// What the proxy's `zbus::Result<T>` holds.
fn ok_type(output: &ReturnType) -> syn::Result<Type> {
    if let ReturnType::Type(_, ty) = output
        && let Type::Path(path) = &**ty
        && let Some(last) = path.path.segments.last()
        && last.ident == "Result"
        && let PathArguments::AngleBracketed(inner) = &last.arguments
        && let Some(GenericArgument::Type(ok)) = inner.args.first()
    {
        return Ok(ok.clone());
    }
    Err(Error::new_spanned(
        output,
        "a bus member answers `zbus::Result<T>`",
    ))
}

/// The type a daemon receives for an argument the proxy borrows: `&str` is a `String`, `&[T]` a
/// `Vec<T>`, any other `&T` a `T`.
fn owned(ty: &Type) -> Type {
    match ty {
        Type::Reference(reference) => match &*reference.elem {
            Type::Path(path) if path.path.is_ident("str") => {
                parse_quote!(::std::string::String)
            }
            Type::Slice(slice) => {
                let element = owned(&slice.elem);
                parse_quote!(::std::vec::Vec<#element>)
            }
            other => other.clone(),
        },
        other => other.clone(),
    }
}
