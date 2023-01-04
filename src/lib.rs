use swc_core::ecma::{
    visit::{VisitWith},
};
use std::collections::HashSet;

use swc_core::{
    common::DUMMY_SP,
    ecma::{
        ast::*,
        utils::ExprFactory,
        visit::{Fold, FoldWith},
    },
    plugin::{
        plugin_transform,
        proxies::TransformPluginProgramMetadata,
    },
};
use swc_core::ecma::utils::quote_ident;

mod tests;
mod utils;
mod builder;
mod tokens;
mod ecma_utils;
mod jsx_visitor;

use builder::*;
use ecma_utils::*;
use jsx_visitor::TransJSXVisitor;
use crate::tokens::{Icu, IcuChoice, MsgToken};

const LINGUI_T: &str = &"t";

fn is_lingui_fn(name: &str) -> bool {
    // todo: i didn't find a better way to create a constant hashmap
    match name {
        "plural" | "select" | "selectOrdinal" => true,
        _ => false,
    }
}

fn is_lingui_jsx_el(name: &str) -> bool {
    // todo: i didn't find a better way to create a constant hashmap
    match name {
        "Plural" | "Select" | "SelectOrdinal" => true,
        _ => false,
    }
}

#[derive(Default)]
pub struct TransformVisitor {
    has_lingui_macro_imports: bool,
    should_add_18n_import: bool,
    should_add_trans_import: bool,
}

 impl TransformVisitor {
    // Receive TemplateLiteral with variables and return MsgTokens
    fn transform_tpl_to_tokens(&self, tpl: &Tpl) -> Vec<MsgToken> {
        let mut tokens: Vec<MsgToken> = Vec::with_capacity(tpl.quasis.len());

        for (i, tpl_element) in tpl.quasis.iter().enumerate() {
            tokens.push(MsgToken::String(tpl_element.raw.to_string()));
            if let Some(exp) = tpl.exprs.get(i) {
                tokens.push(MsgToken::Expression(exp.clone()));
            }
        }

        tokens
    }

    fn create_i18n_fn_call(&self, callee_obj: Box<Expr>, tokens: Vec<MsgToken>) -> CallExpr {
        let parsed = MessageBuilder::parse(tokens);

        let mut args: Vec<ExprOrSpread> = vec![parsed.message.as_arg()];

        if let Some(v) = parsed.values {
            args.push(v.as_arg())
        }

        return CallExpr {
            span: DUMMY_SP,
            callee: Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: callee_obj,
                prop: MemberProp::Ident(Ident::new("_".into(), DUMMY_SP)),
            }).as_callee(),
            args,
            type_args: None,
        };
    }

    // receive ObjectLiteral {few: "..", many: "..", other: ".."} and create tokens
    // If messages passed as TemplateLiterals with variables, it extracts variables
    fn get_choices_from_obj(&self, props: &Vec<PropOrSpread>) -> Vec<IcuChoice> {
        // todo: there might be more props then real choices. Id for example
        let mut choices: Vec<IcuChoice> = Vec::with_capacity(props.len());

        for prop_or_spread in props {
            if let PropOrSpread::Prop(prop) = prop_or_spread {
                if let Prop::KeyValue(prop) = prop.as_ref() {
                    if let PropName::Ident(ident) = &prop.key {
                        let mut tokens: Vec<MsgToken> = Vec::new();

                        // String Literal: "has # friend"
                        if let Expr::Lit(lit) = prop.value.as_ref() {
                            if let Lit::Str(str) = lit {
                                tokens = vec!(MsgToken::String(str.clone().value.to_string()));
                            }
                        }

                        // Template Literal: `${name} has # friend`
                        if let Expr::Tpl(tpl) = prop.value.as_ref() {
                            tokens = self.transform_tpl_to_tokens(tpl);
                        }

                        choices.push(IcuChoice {
                            tokens,
                            key: ident.sym.to_string(),
                        })
                    } else {
                        // todo panic
                    }
                    // icuParts.push_str(prop.key)
                } else {
                    // todo: panic here we could not parse anything else then KeyValue pair
                }
            } else {
                // todo: panic here, we could not parse spread
            }
        }

        choices
    }

    // <Trans>Message</Trans>
    // <Plural />
    fn transform_jsx_macro(&mut self, el: JSXElement, is_trans_el: bool) -> JSXElement {
        let mut trans_visitor = TransJSXVisitor::new();

        if is_trans_el {
            el.children.visit_children_with(&mut trans_visitor);
        } else {
            el.visit_children_with(&mut trans_visitor);
        }

        let parsed = MessageBuilder::parse(trans_visitor.tokens);
        let id_attr = get_jsx_attr(&el.opening, "id");

        let mut attrs = vec![
            create_jsx_attribute(
                if let Some(_) = id_attr { "message" } else { "id" }.into(),
                parsed.message,
            ),
        ];

        if let Some(exp) = parsed.values {
            attrs.push(create_jsx_attribute(
                "values",
                exp,
            ));
        }

        if let Some(exp) = parsed.components {
            attrs.push(create_jsx_attribute(
                "components",
                exp,
            ));
        }

        attrs.extend(
            pick_jsx_attrs(el.opening.attrs, HashSet::from(["id", "render"]))
        );

        self.should_add_trans_import = true;

        return JSXElement {
            span: el.span,
            children: vec![],
            closing: None,
            opening: JSXOpeningElement {
                self_closing: true,
                span: el.opening.span,
                name: JSXElementName::Ident(
                    Ident::new("Trans".into(), el.opening.span)
                ),
                type_args: None,
                attrs,
            },
        };
    }
}


impl Fold for TransformVisitor {
    fn fold_module_items(&mut self, mut n: Vec<ModuleItem>) -> Vec<ModuleItem> {
        let mut has_i18n_import = false;
        let mut has_trans_import = false;

        n.retain(|m| {
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(imp)) = m {
                // drop macro imports
                if &imp.src.value == "@lingui/macro" {
                    self.has_lingui_macro_imports = true;
                    return false;
                }

                if &imp.src.value == "@lingui/core" && !imp.type_only {
                    for spec in &imp.specifiers {
                        if let ImportSpecifier::Named(spec) = spec {
                            has_i18n_import = if !has_i18n_import { &spec.local.sym == "i18n" } else { true };
                        }
                    }
                }

                if &imp.src.value == "@lingui/react" && !imp.type_only {
                    for spec in &imp.specifiers {
                        if let ImportSpecifier::Named(spec) = spec {
                            has_trans_import = if !has_trans_import { &spec.local.sym == "Trans" } else { true };
                        }
                    }
                }
            }

            true
        });

        n = n.fold_children_with(self);

        if !has_i18n_import && self.should_add_18n_import {
            n.insert(0, create_import("@lingui/core".into(), quote_ident!("i18n")));
        }

        if !has_trans_import && self.should_add_trans_import {
            n.insert(0, create_import("@lingui/react".into(), quote_ident!("Trans")));
        }

        n
    }

    fn fold_expr(&mut self, expr: Expr) -> Expr {
        // If no package that we care about is imported, skip the following
        // transformation logic.
        if !self.has_lingui_macro_imports {
            return expr;
        }

        if let Expr::TaggedTpl(tagged_tpl) = &expr {
            match tagged_tpl.tag.as_ref() {
                // t(i18n)``
                Expr::Call(call) if match_callee_name(call, LINGUI_T) => {
                    if let Some(v) = call.args.get(0) {
                        return Expr::Call(self.create_i18n_fn_call(
                            v.expr.clone(),
                            self.transform_tpl_to_tokens(&tagged_tpl.tpl),
                        ));
                    }
                }
                // t``
                Expr::Ident(ident) if &ident.sym == LINGUI_T => {
                    self.should_add_18n_import = true;
                    return Expr::Call(self.create_i18n_fn_call(
                        Box::new(Ident::new("i18n".into(), DUMMY_SP).into()),
                        self.transform_tpl_to_tokens(&tagged_tpl.tpl),
                    ));
                }
                _ => {}
            }
        }

        expr.fold_children_with(self)
    }

    fn fold_call_expr(&mut self, expr: CallExpr) -> CallExpr {
        // If no package that we care about is imported, skip the following
        // transformation logic.
        if !self.has_lingui_macro_imports {
            return expr;
        }

        if let Callee::Expr(e) = &expr.callee {
            match e.as_ref() {
                // (plural | select | selectOrdinal)()
                Expr::Ident(ident) => {
                    if !is_lingui_fn(&ident.sym) {
                        return expr;
                    }

                    if expr.args.len() != 2 {
                        // malformed plural call, exit
                        return expr;
                    }


                    // ICU value
                    let arg = expr.args.get(0).unwrap();
                    let icu_value = arg.expr.clone();


                    // ICU Choices
                    let arg = expr.args.get(1).unwrap();
                    if let Expr::Object(object) = &arg.expr.as_ref() {
                        let choices = self.get_choices_from_obj(&object.props);

                        self.should_add_18n_import = true;

                        return self.create_i18n_fn_call(
                            Box::new(Ident::new("i18n".into(), DUMMY_SP).into()),
                            vec!(MsgToken::Icu(Icu {
                                icu_method: ident.sym.clone().to_string(),
                                value: icu_value,
                                choices,
                            })),
                        );
                    } else {
                        // todo passed not an ObjectLiteral,
                        //      we should panic here or just skip this call
                    }
                }
                _ => {}
            }
        }

        expr
    }

    fn fold_jsx_element(&mut self, el: JSXElement) -> JSXElement {
        // If no package that we care about is imported, skip the following
        // transformation logic.
        if !self.has_lingui_macro_imports {
            return el;
        }

        if let JSXElementName::Ident(ident) = &el.opening.name {
            if &ident.sym == "Trans" {
                return self.transform_jsx_macro(el, true);
            }

            if is_lingui_jsx_el(&ident.sym) {
                return self.transform_jsx_macro(el, false);
            }
        }

        el
    }
}


/// An example plugin function with macro support.
/// `plugin_transform` macro interop pointers into deserialized structs, as well
/// as returning ptr back to host.
///
/// It is possible to opt out from macro by writing transform fn manually
/// if plugin need to handle low-level ptr directly via
/// `__transform_plugin_process_impl(
///     ast_ptr: *const u8, ast_ptr_len: i32,
///     unresolved_mark: u32, should_enable_comments_proxy: i32) ->
///     i32 /*  0 for success, fail otherwise.
///             Note this is only for internal pointer interop result,
///             not actual transform result */`
///
/// This requires manual handling of serialization / deserialization from ptrs.
/// Refer swc_plugin_macro to see how does it work internally.
#[plugin_transform]
pub fn process_transform(program: Program, _metadata: TransformPluginProgramMetadata) -> Program {
    program.fold_with(&mut TransformVisitor::default())
}

