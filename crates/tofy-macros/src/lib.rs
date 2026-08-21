use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

/// Turns a stack declaration `fn main` into a tofy program.
///
/// Call `stack(...).apply()` in `main`. That is apply. Pass `plan`, `destroy`,
/// `output`, `run`, or `emit` after `--` to choose another command.
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);
    let vis = func.vis.clone();
    let asyncness = &func.sig.asyncness;
    if asyncness.is_some() {
        return syn::Error::new_spanned(asyncness, "#[tofy::main] does not support async fn")
            .to_compile_error()
            .into();
    }

    let user_ident = syn::Ident::new("__tofy_user_main", func.sig.ident.span());
    func.sig.ident = user_ident.clone();

    quote! {
        #func

        #vis fn main() {
            #user_ident();
            if let Err(e) = ::tofy::rt::finish() {
                eprintln!("tofy: {e}");
                ::std::process::exit(e.exit_code());
            }
        }
    }
    .into()
}
