//! Doctest functionality used only for doctests in `.rs` source files.

use std::env;

use rustc_data_structures::{fx::FxHashSet, sync::Lrc};
use rustc_hir::def_id::{LocalDefId, CRATE_DEF_ID};
use rustc_hir::{self as hir, intravisit, CRATE_HIR_ID};
use rustc_middle::hir::map::Map;
use rustc_middle::hir::nested_filter;
use rustc_middle::ty::TyCtxt;
use rustc_resolve::rustdoc::span_of_fragments;
use rustc_session::Session;
use rustc_span::source_map::SourceMap;
use rustc_span::{BytePos, FileName, Pos, Span, DUMMY_SP};

use super::DoctestVisitor;
use crate::clean::{types::AttributesExt, Attributes};
use crate::html::markdown::{self, ErrorCodes, LangString};

pub(super) struct RustDoctest {
    pub(super) filename: FileName,
    pub(super) line: usize,
    pub(super) logical_path: Vec<String>,
    pub(super) langstr: LangString,
    pub(super) text: String,
}

struct RustCollector {
    source_map: Lrc<SourceMap>,
    tests: Vec<RustDoctest>,
    cur_path: Vec<String>,
    position: Span,
}

impl RustCollector {
    fn get_filename(&self) -> FileName {
        let filename = self.source_map.span_to_filename(self.position);
        if let FileName::Real(ref filename) = filename
            && let Ok(cur_dir) = env::current_dir()
            && let Some(local_path) = filename.local_path()
            && let Ok(path) = local_path.strip_prefix(&cur_dir)
        {
            return path.to_owned().into();
        }
        filename
    }
}

impl DoctestVisitor for RustCollector {
    fn visit_test(&mut self, test: String, config: LangString, line: usize) {
        self.tests.push(RustDoctest {
            filename: self.get_filename(),
            line,
            logical_path: self.cur_path.clone(),
            langstr: config,
            text: test,
        });
    }

    fn get_line(&self) -> usize {
        let line = self.position.lo().to_usize();
        let line = self.source_map.lookup_char_pos(BytePos(line as u32)).line;
        if line > 0 { line - 1 } else { line }
    }

    fn visit_header(&mut self, _name: &str, _level: u32) {}
}

pub(super) struct HirCollector<'a, 'tcx> {
    sess: &'a Session,
    map: Map<'tcx>,
    codes: ErrorCodes,
    tcx: TyCtxt<'tcx>,
    enable_per_target_ignores: bool,
    collector: RustCollector,
}

impl<'a, 'tcx> HirCollector<'a, 'tcx> {
    pub fn new(
        sess: &'a Session,
        map: Map<'tcx>,
        codes: ErrorCodes,
        enable_per_target_ignores: bool,
        tcx: TyCtxt<'tcx>,
    ) -> Self {
        let collector = RustCollector {
            source_map: sess.psess.clone_source_map(),
            cur_path: vec![],
            position: DUMMY_SP,
            tests: vec![],
        };
        Self { sess, map, codes, enable_per_target_ignores, tcx, collector }
    }

    pub fn collect_crate(mut self) -> Vec<RustDoctest> {
        let tcx = self.tcx;
        self.visit_testable("".to_string(), CRATE_DEF_ID, tcx.hir().span(CRATE_HIR_ID), |this| {
            tcx.hir().walk_toplevel_module(this)
        });
        self.collector.tests
    }
}

impl<'a, 'tcx> HirCollector<'a, 'tcx> {
    fn visit_testable<F: FnOnce(&mut Self)>(
        &mut self,
        name: String,
        def_id: LocalDefId,
        sp: Span,
        nested: F,
    ) {
        let ast_attrs = self.tcx.hir().attrs(self.tcx.local_def_id_to_hir_id(def_id));
        if let Some(ref cfg) = ast_attrs.cfg(self.tcx, &FxHashSet::default()) {
            if !cfg.matches(&self.sess.psess, Some(self.tcx.features())) {
                return;
            }
        }

        let has_name = !name.is_empty();
        if has_name {
            self.collector.cur_path.push(name);
        }

        // The collapse-docs pass won't combine sugared/raw doc attributes, or included files with
        // anything else, this will combine them for us.
        let attrs = Attributes::from_ast(ast_attrs);
        if let Some(doc) = attrs.opt_doc_value() {
            // Use the outermost invocation, so that doctest names come from where the docs were written.
            let span = ast_attrs
                .iter()
                .find(|attr| attr.doc_str().is_some())
                .map(|attr| attr.span.ctxt().outer_expn().expansion_cause().unwrap_or(attr.span))
                .unwrap_or(DUMMY_SP);
            self.collector.position = span;
            markdown::find_testable_code(
                &doc,
                &mut self.collector,
                self.codes,
                self.enable_per_target_ignores,
                Some(&crate::html::markdown::ExtraInfo::new(
                    self.tcx,
                    def_id.to_def_id(),
                    span_of_fragments(&attrs.doc_strings).unwrap_or(sp),
                )),
            );
        }

        nested(self);

        if has_name {
            self.collector.cur_path.pop();
        }
    }
}

impl<'a, 'tcx> intravisit::Visitor<'tcx> for HirCollector<'a, 'tcx> {
    type NestedFilter = nested_filter::All;

    fn nested_visit_map(&mut self) -> Self::Map {
        self.map
    }

    fn visit_item(&mut self, item: &'tcx hir::Item<'_>) {
        let name = match &item.kind {
            hir::ItemKind::Impl(impl_) => {
                rustc_hir_pretty::id_to_string(&self.map, impl_.self_ty.hir_id)
            }
            _ => item.ident.to_string(),
        };

        self.visit_testable(name, item.owner_id.def_id, item.span, |this| {
            intravisit::walk_item(this, item);
        });
    }

    fn visit_trait_item(&mut self, item: &'tcx hir::TraitItem<'_>) {
        self.visit_testable(item.ident.to_string(), item.owner_id.def_id, item.span, |this| {
            intravisit::walk_trait_item(this, item);
        });
    }

    fn visit_impl_item(&mut self, item: &'tcx hir::ImplItem<'_>) {
        self.visit_testable(item.ident.to_string(), item.owner_id.def_id, item.span, |this| {
            intravisit::walk_impl_item(this, item);
        });
    }

    fn visit_foreign_item(&mut self, item: &'tcx hir::ForeignItem<'_>) {
        self.visit_testable(item.ident.to_string(), item.owner_id.def_id, item.span, |this| {
            intravisit::walk_foreign_item(this, item);
        });
    }

    fn visit_variant(&mut self, v: &'tcx hir::Variant<'_>) {
        self.visit_testable(v.ident.to_string(), v.def_id, v.span, |this| {
            intravisit::walk_variant(this, v);
        });
    }

    fn visit_field_def(&mut self, f: &'tcx hir::FieldDef<'_>) {
        self.visit_testable(f.ident.to_string(), f.def_id, f.span, |this| {
            intravisit::walk_field_def(this, f);
        });
    }
}
