use crate::model::{GraphName, NamedOrBlankNode};
use crate::sparql::algebra::DatasetSpec;
use crate::sparql::EvaluationError;
use crate::store::numeric_encoder::{
    EncodedQuad, EncodedTerm, ReadEncoder, StrContainer, StrEncodingAware, StrId, StrLookup,
};
use crate::store::ReadableEncodedStore;
use lasso::{Rodeo, Spur};
use std::cell::RefCell;
use std::iter::empty;

pub(crate) struct DatasetView<S: ReadableEncodedStore> {
    store: S,
    extra: RefCell<Rodeo>,
    default_graph_as_union: bool,
    dataset: Option<EncodedDatasetSpec<S::StrId>>,
}

impl<S: ReadableEncodedStore> DatasetView<S> {
    pub fn new(
        store: S,
        default_graph_as_union: bool,
        default_graphs: &[GraphName],
        named_graphs: &[NamedOrBlankNode],
        dataset: &DatasetSpec,
    ) -> Result<Self, EvaluationError> {
        let dataset = if !default_graphs.is_empty() || !named_graphs.is_empty() {
            Some(EncodedDatasetSpec {
                default: default_graphs
                    .iter()
                    .flat_map(|g| store.get_encoded_graph_name(g.as_ref()).transpose())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.into())?,
                named: named_graphs
                    .iter()
                    .flat_map(|g| {
                        store
                            .get_encoded_named_or_blank_node(g.as_ref())
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.into())?,
            })
        } else if dataset.is_empty() {
            None
        } else {
            Some(EncodedDatasetSpec {
                default: dataset
                    .default
                    .iter()
                    .flat_map(|g| store.get_encoded_named_node(g.as_ref()).transpose())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.into())?,
                named: dataset
                    .named
                    .iter()
                    .flat_map(|g| store.get_encoded_named_node(g.as_ref()).transpose())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.into())?,
            })
        };
        Ok(Self {
            store,
            extra: RefCell::new(Rodeo::default()),
            default_graph_as_union,
            dataset,
        })
    }

    fn encoded_quads_for_pattern_in_dataset(
        &self,
        subject: Option<EncodedTerm<S::StrId>>,
        predicate: Option<EncodedTerm<S::StrId>>,
        object: Option<EncodedTerm<S::StrId>>,
        graph_name: Option<EncodedTerm<S::StrId>>,
    ) -> Box<dyn Iterator<Item = Result<EncodedQuad<DatasetStrId<S::StrId>>, EvaluationError>>>
    {
        if let Some(dataset) = &self.dataset {
            if let Some(graph_name) = graph_name {
                if graph_name == EncodedTerm::DefaultGraph {
                    let iters = dataset
                        .default
                        .iter()
                        .map(|graph_name| {
                            self.store.encoded_quads_for_pattern(
                                subject,
                                predicate,
                                object,
                                Some(*graph_name),
                            )
                        })
                        .collect::<Vec<_>>();
                    Box::new(map_iter(iters.into_iter().flatten()).map(|quad| {
                        let quad = quad?;
                        Ok(EncodedQuad::new(
                            quad.subject,
                            quad.predicate,
                            quad.object,
                            EncodedTerm::DefaultGraph,
                        ))
                    }))
                } else if dataset.named.contains(&graph_name) {
                    Box::new(map_iter(self.store.encoded_quads_for_pattern(
                        subject,
                        predicate,
                        object,
                        Some(graph_name),
                    )))
                } else {
                    Box::new(empty())
                }
            } else {
                let iters = dataset
                    .named
                    .iter()
                    .map(|graph_name| {
                        self.store.encoded_quads_for_pattern(
                            subject,
                            predicate,
                            object,
                            Some(*graph_name),
                        )
                    })
                    .collect::<Vec<_>>();
                Box::new(map_iter(iters.into_iter().flatten()))
            }
        } else if graph_name == None {
            Box::new(
                map_iter(
                    self.store
                        .encoded_quads_for_pattern(subject, predicate, object, None),
                )
                .filter(|quad| match quad {
                    Err(_) => true,
                    Ok(quad) => quad.graph_name != EncodedTerm::DefaultGraph,
                }),
            )
        } else {
            Box::new(map_iter(self.store.encoded_quads_for_pattern(
                subject, predicate, object, graph_name,
            )))
        }
    }
}

impl<S: ReadableEncodedStore> StrEncodingAware for DatasetView<S> {
    type Error = EvaluationError;
    type StrId = DatasetStrId<S::StrId>;
}

impl<S: ReadableEncodedStore> StrLookup for DatasetView<S> {
    fn get_str(&self, id: DatasetStrId<S::StrId>) -> Result<Option<String>, EvaluationError> {
        match id {
            DatasetStrId::Store(id) => self.store.get_str(id).map_err(|e| e.into()),
            DatasetStrId::Temporary(id) => {
                Ok(self.extra.borrow().try_resolve(&id).map(|e| e.to_owned()))
            }
        }
    }

    fn get_str_id(&self, value: &str) -> Result<Option<DatasetStrId<S::StrId>>, EvaluationError> {
        if let Some(id) = self.extra.borrow().get(value) {
            Ok(Some(DatasetStrId::Temporary(id)))
        } else {
            Ok(self
                .store
                .get_str_id(value)
                .map_err(|e| e.into())?
                .map(DatasetStrId::Store))
        }
    }
}

impl<S: ReadableEncodedStore> ReadableEncodedStore for DatasetView<S> {
    type QuadsIter =
        Box<dyn Iterator<Item = Result<EncodedQuad<DatasetStrId<S::StrId>>, EvaluationError>>>;

    fn encoded_quads_for_pattern(
        &self,
        subject: Option<EncodedTerm<Self::StrId>>,
        predicate: Option<EncodedTerm<Self::StrId>>,
        object: Option<EncodedTerm<Self::StrId>>,
        graph_name: Option<EncodedTerm<Self::StrId>>,
    ) -> Box<dyn Iterator<Item = Result<EncodedQuad<DatasetStrId<S::StrId>>, EvaluationError>>>
    {
        if let Some((subject, predicate, object, graph_name)) =
            try_map_quad_pattern(subject, predicate, object, graph_name)
        {
            if graph_name == Some(EncodedTerm::DefaultGraph) && self.default_graph_as_union {
                Box::new(
                    self.encoded_quads_for_pattern_in_dataset(
                        subject,
                        predicate,
                        object,
                        Some(EncodedTerm::DefaultGraph),
                    )
                    .chain(
                        self.encoded_quads_for_pattern_in_dataset(subject, predicate, object, None),
                    ),
                )
            } else {
                self.encoded_quads_for_pattern_in_dataset(subject, predicate, object, graph_name)
            }
        } else {
            Box::new(empty())
        }
    }
}

fn map_iter<'a, I: StrId>(
    iter: impl Iterator<Item = Result<EncodedQuad<I>, impl Into<EvaluationError>>> + 'a,
) -> impl Iterator<Item = Result<EncodedQuad<DatasetStrId<I>>, EvaluationError>> + 'a {
    iter.map(|t| {
        t.map(|q| EncodedQuad {
            subject: q.subject.map_id(DatasetStrId::Store),
            predicate: q.predicate.map_id(DatasetStrId::Store),
            object: q.object.map_id(DatasetStrId::Store),
            graph_name: q.graph_name.map_id(DatasetStrId::Store),
        })
        .map_err(|e| e.into())
    })
}

type QuadPattern<I> = (
    Option<EncodedTerm<I>>,
    Option<EncodedTerm<I>>,
    Option<EncodedTerm<I>>,
    Option<EncodedTerm<I>>,
);

fn try_map_quad_pattern<I: StrId>(
    subject: Option<EncodedTerm<DatasetStrId<I>>>,
    predicate: Option<EncodedTerm<DatasetStrId<I>>>,
    object: Option<EncodedTerm<DatasetStrId<I>>>,
    graph_name: Option<EncodedTerm<DatasetStrId<I>>>,
) -> Option<QuadPattern<I>> {
    Some((
        transpose(subject.map(|t| t.try_map_id(unwrap_store_id).ok()))?,
        transpose(predicate.map(|t| t.try_map_id(unwrap_store_id).ok()))?,
        transpose(object.map(|t| t.try_map_id(unwrap_store_id).ok()))?,
        transpose(graph_name.map(|t| t.try_map_id(unwrap_store_id).ok()))?,
    ))
}

fn transpose<T>(o: Option<Option<T>>) -> Option<Option<T>> {
    match o {
        Some(Some(v)) => Some(Some(v)),
        Some(None) => None,
        None => Some(None),
    }
}

fn unwrap_store_id<I: StrId>(id: DatasetStrId<I>) -> Result<I, ()> {
    match id {
        DatasetStrId::Store(id) => Ok(id),
        DatasetStrId::Temporary(_) => Err(()),
    }
}

impl<'a, S: ReadableEncodedStore> StrContainer for &'a DatasetView<S> {
    fn insert_str(&mut self, value: &str) -> Result<Self::StrId, EvaluationError> {
        if let Some(id) = self.store.get_str_id(value).map_err(|e| e.into())? {
            Ok(DatasetStrId::Store(id))
        } else {
            Ok(DatasetStrId::Temporary(
                self.extra.borrow_mut().get_or_intern(value),
            ))
        }
    }
}

#[derive(Eq, PartialEq, Debug, Copy, Clone, Hash)]
pub enum DatasetStrId<I: StrId> {
    Store(I),
    Temporary(Spur),
}

impl<I: StrId> StrId for DatasetStrId<I> {}

struct EncodedDatasetSpec<I: StrId> {
    default: Vec<EncodedTerm<I>>,
    named: Vec<EncodedTerm<I>>,
}
