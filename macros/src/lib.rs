#![deny(warnings)]

extern crate proc_macro;

use core::mem;
use proc_macro::TokenStream;
use std::borrow::Cow;

use proc_macro2::Span;
use quote::quote;
use syn::{
    parse::{self, Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Data, DeriveInput, Expr, Fields, Ident, IntSuffix, LitInt, LitStr, Token,
};

#[proc_macro_derive(uDebug)]
pub fn debug(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let ident = &input.ident;
    let ts = match input.data {
        Data::Struct(data) => {
            let ident_s = ident.to_string();

            let body = match data.fields {
                Fields::Named(fields) => {
                    let fields = fields
                        .named
                        .iter()
                        .map(|field| {
                            let ident = field.ident.as_ref().expect("UNREACHABLE");
                            let name = ident.to_string();

                            quote!(field(#name, &self.#ident)?)
                        })
                        .collect::<Vec<_>>();

                    quote!(f.debug_struct(#ident_s)?#(.#fields)*.finish())
                }

                Fields::Unnamed(fields) => {
                    let fields = (0..fields.unnamed.len())
                        .map(|i| {
                            let i = LitInt::new(i as u64, IntSuffix::None, Span::call_site());

                            quote!(field(&self.#i)?)
                        })
                        .collect::<Vec<_>>();

                    quote!(f.debug_tuple(#ident_s)?#(.#fields)*.finish())
                }

                Fields::Unit => quote!(f.write_str(#ident_s)),
            };

            quote!(
                impl ufmt::uDebug for #ident {
                    fn fmt<W>(&self, f: &mut ufmt::Formatter<'_, W>) -> Result<(), W::Error>
                    where
                        W: ufmt::uWrite,
                    {
                        #body
                    }
                }

            )
        }

        Data::Enum(data) => {
            let arms = data
                .variants
                .iter()
                .map(|var| {
                    let variant = &var.ident;
                    let variant_s = variant.to_string();

                    match &var.fields {
                        Fields::Named(fields) => {
                            let mut pats = Vec::with_capacity(fields.named.len());
                            let mut methods = Vec::with_capacity(fields.named.len());
                            for field in &fields.named {
                                let ident = field.ident.as_ref().unwrap();
                                let ident_s = ident.to_string();

                                pats.push(quote!(#ident));
                                methods.push(quote!(field(#ident_s, #ident)?));
                            }

                            quote!(
                                #ident::#variant { #(#pats),* } => {
                                    f.debug_struct(#variant_s)?#(.#methods)*.finish()
                                }
                            )
                        }

                        Fields::Unnamed(fields) => {
                            let pats = &(0..fields.unnamed.len())
                                .map(|i| Ident::new(&format!("_{}", i), Span::call_site()))
                                .collect::<Vec<_>>();

                            quote!(
                                #ident::#variant(#(#pats),*) => {
                                    f.debug_tuple(#variant_s)?#(.field(#pats)?)*.finish()
                                }
                            )
                        }

                        Fields::Unit => quote!(
                            #ident::#variant => {
                                f.write_str(#variant_s)
                            }
                        ),
                    }
                })
                .collect::<Vec<_>>();

            quote!(
                impl ufmt::uDebug for #ident {
                    fn fmt<W>(&self, f: &mut ufmt::Formatter<'_, W>) -> Result<(), W::Error>
                        where
                        W: ufmt::uWrite,
                    {
                        match self {
                            #(#arms),*
                        }
                    }
                }
            )
        }

        Data::Union(..) => {
            return parse::Error::new(Span::call_site(), "this trait cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    ts.into()
}

#[proc_macro]
pub fn uwrite(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as Input);

    let formatter = &input.formatter;
    let literal = input.literal;

    let s = literal.value();
    let pieces = match parse(&s, literal.span()) {
        Err(e) => return e.to_compile_error().into(),
        Ok(pieces) => pieces,
    };
    let mut args = input.args.iter();

    let exprs = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Str(s) => quote!(ufmt::uDisplay::fmt(#s, f)?;),

            Piece::Display => {
                let arg = args.next().expect("FIXME");
                quote!(ufmt::uDisplay::fmt(&(#arg), f)?;)
            }

            Piece::Debug { pretty } => {
                let arg = args.next().expect("FIXME");

                if *pretty {
                    quote!(f.pretty(|f| ufmt::uDebug::fmt(&(#arg), f))?;)
                } else {
                    quote!(ufmt::uDebug::fmt(&(#arg), f)?;)
                }
            }
        })
        .collect::<Vec<_>>();

    quote!(ufmt::do_(#formatter, |f| {
        #(#exprs)*
        Ok(())
    }))
    .into()
}

struct Input {
    formatter: Expr,
    _comma: Token![,],
    literal: LitStr,
    _comma2: Token![,],
    args: Punctuated<Expr, Token![,]>,
}

impl Parse for Input {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        Ok(Input {
            formatter: input.parse()?,
            _comma: input.parse()?,
            literal: input.parse()?,
            _comma2: input.parse()?,
            args: Punctuated::parse_terminated(input)?,
        })
    }
}

#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Debug { pretty: bool },
    Display,
    Str(Cow<'a, str>),
}

// `}}` -> `}`
fn unescape<'l>(mut literal: &'l str, span: Span) -> parse::Result<Cow<'l, str>> {
    if literal.contains('}') {
        let mut buf = String::new();

        while literal.contains('}') {
            const ERR: &str = "format string contains an unmatched right brace";
            let mut parts = literal.splitn(2, '}');

            match (parts.next(), parts.next()) {
                (Some(left), Some(right)) => {
                    const ESCAPED_BRACE: &str = "}";

                    if right.starts_with(ESCAPED_BRACE) {
                        buf.push_str(left);
                        buf.push('}');

                        literal = &right[ESCAPED_BRACE.len()..];
                    } else {
                        return Err(parse::Error::new(span, ERR));
                    }
                }

                _ => unreachable!(),
            }
        }

        buf.push_str(literal);

        Ok(buf.into())
    } else {
        Ok(Cow::Borrowed(literal))
    }
}

fn parse<'l>(mut literal: &'l str, span: Span) -> parse::Result<Vec<Piece<'l>>> {
    let mut pieces = vec![];

    let mut buf = String::new();
    loop {
        let mut parts = literal.splitn(2, '{');
        match (parts.next(), parts.next()) {
            // empty string literal
            (None, None) => break,

            // end of the string literal
            (Some(s), None) => {
                if buf.is_empty() {
                    if !s.is_empty() {
                        pieces.push(Piece::Str(unescape(s, span)?));
                    }
                } else {
                    buf.push_str(&unescape(s, span)?);

                    pieces.push(Piece::Str(Cow::Owned(buf)));
                }

                break;
            }

            (head, Some(tail)) => {
                const DEBUG: &str = ":?}";
                const DEBUG_PRETTY: &str = ":#?}";
                const DISPLAY: &str = "}";
                const ESCAPED_BRACE: &str = "{";

                let head = head.unwrap_or("");
                if tail.starts_with(DEBUG)
                    || tail.starts_with(DEBUG_PRETTY)
                    || tail.starts_with(DISPLAY)
                {
                    if buf.is_empty() {
                        if !head.is_empty() {
                            pieces.push(Piece::Str(unescape(head, span)?));
                        }
                    } else {
                        buf.push_str(&unescape(head, span)?);

                        pieces.push(Piece::Str(Cow::Owned(mem::replace(
                            &mut buf,
                            String::new(),
                        ))));
                    }

                    if tail.starts_with(DEBUG) {
                        pieces.push(Piece::Debug { pretty: false });

                        literal = &tail[DEBUG.len()..];
                    } else if tail.starts_with(DEBUG_PRETTY) {
                        pieces.push(Piece::Debug { pretty: true });

                        literal = &tail[DEBUG_PRETTY.len()..];
                    } else {
                        pieces.push(Piece::Display);

                        literal = &tail[DISPLAY.len()..];
                    }
                } else if tail.starts_with(ESCAPED_BRACE) {
                    buf.push_str(&unescape(head, span)?);
                    buf.push('{');

                    literal = &tail[ESCAPED_BRACE.len()..];
                } else {
                    return Err(parse::Error::new(
                        span,
                        "invalid format string: expected `{{`, `{}`, `{:?}` or `{:#?}`",
                    ));
                }
            }
        }
    }

    Ok(pieces)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use proc_macro2::Span;

    use crate::Piece;

    #[test]
    fn pieces() {
        let span = Span::call_site();

        // string interpolation
        assert_eq!(
            super::parse("The answer is {}", span).ok(),
            Some(vec![
                Piece::Str(Cow::Borrowed("The answer is ")),
                Piece::Display
            ]),
        );

        assert_eq!(
            super::parse("{:?}", span).ok(),
            Some(vec![Piece::Debug { pretty: false }]),
        );

        assert_eq!(
            super::parse("{:#?}", span).ok(),
            Some(vec![Piece::Debug { pretty: true }]),
        );

        // escaped braces
        assert_eq!(
            super::parse("{{}} is not an argument", span).ok(),
            Some(vec![Piece::Str(Cow::Borrowed("{} is not an argument"))]),
        );

        // left brace & junk
        assert!(super::parse("{", span).is_err());
        assert!(super::parse(" {", span).is_err());
        assert!(super::parse("{ ", span).is_err());
        assert!(super::parse("{ {", span).is_err());
        assert!(super::parse("{:x}", span).is_err());
    }

    #[test]
    fn unescape() {
        let span = Span::call_site();

        // no right brace
        assert_eq!(super::unescape("", span).ok(), Some(Cow::Borrowed("")));
        assert_eq!(
            super::unescape("Hello", span).ok(),
            Some(Cow::Borrowed("Hello"))
        );

        // unmatched right brace
        assert!(super::unescape(" }", span).is_err());
        assert!(super::unescape("} ", span).is_err());
        assert!(super::unescape("}", span).is_err());

        // escaped right brace
        assert_eq!(super::unescape("}}", span).ok(), Some(Cow::Borrowed("}")));
        assert_eq!(super::unescape("}} ", span).ok(), Some(Cow::Borrowed("} ")));
    }
}
