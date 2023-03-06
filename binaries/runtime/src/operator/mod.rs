use dora_core::{
    config::{DataId, NodeId},
    descriptor::{OperatorDefinition, OperatorSource},
    message::{Metadata, MetadataParameters},
};
use dora_operator_api_python::metadata_to_pydict;
use eyre::Context;
#[cfg(feature = "tracing")]
use opentelemetry::sdk::trace::Tracer;
use pyo3::{
    types::{PyBytes, PyDict},
    IntoPy, PyObject, Python,
};
use std::any::Any;
use tokio::sync::mpsc::Sender;

#[cfg(not(feature = "tracing"))]
type Tracer = ();

pub mod channel;
mod python;
mod shared_lib;

pub fn run_operator(
    node_id: &NodeId,
    operator_definition: OperatorDefinition,
    incoming_events: flume::Receiver<IncomingEvent>,
    events_tx: Sender<OperatorEvent>,
) -> eyre::Result<()> {
    #[cfg(feature = "tracing")]
    let tracer =
        dora_tracing::init_tracing(format!("{node_id}/{}", operator_definition.id).as_str())
            .wrap_err("could not initiate tracing for operator")?;
    #[cfg(not(feature = "tracing"))]
    #[allow(clippy::let_unit_value)]
    let tracer = ();

    match &operator_definition.config.source {
        OperatorSource::SharedLibrary(source) => {
            shared_lib::run(
                node_id,
                &operator_definition.id,
                source,
                events_tx,
                incoming_events,
                tracer,
            )
            .wrap_err_with(|| {
                format!(
                    "failed to spawn shared library operator for {}",
                    operator_definition.id
                )
            })?;
        }
        OperatorSource::Python(source) => {
            python::run(
                node_id,
                &operator_definition.id,
                source,
                events_tx,
                incoming_events,
                tracer,
            )
            .wrap_err_with(|| {
                format!(
                    "failed to spawn Python operator for {}",
                    operator_definition.id
                )
            })?;
        }
        OperatorSource::Wasm(_) => {
            tracing::error!("WASM operators are not supported yet");
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum OperatorEvent {
    Output {
        output_id: DataId,
        metadata: MetadataParameters<'static>,
        data: Vec<u8>,
    },
    Error(eyre::Error),
    Panic(Box<dyn Any + Send>),
    Finished {
        reason: StopReason,
    },
}

#[derive(Debug)]
pub enum IncomingEvent {
    Stop,
    Input {
        input_id: DataId,
        metadata: Metadata<'static>,
        data: Option<Vec<u8>>,
    },
    InputClosed {
        input_id: DataId,
    },
}

impl IntoPy<PyObject> for IncomingEvent {
    fn into_py(self, py: Python) -> PyObject {
        let dict = PyDict::new(py);

        let ty = match self {
            Self::Stop => "STOP",
            Self::Input {
                input_id,
                metadata,
                data,
            } => {
                dict.set_item("id", input_id.to_string())
                    .wrap_err("failed to add input ID")
                    .unwrap();
                dict.set_item(
                    "data",
                    PyBytes::new(py, data.as_deref().unwrap_or_default()),
                )
                .wrap_err("failed to add input data")
                .unwrap();
                dict.set_item("metadata", metadata_to_pydict(&metadata, py))
                    .wrap_err("failed to add input metadata")
                    .unwrap();
                "INPUT"
            }
            Self::InputClosed { input_id } => {
                dict.set_item("id", input_id.to_string())
                    .wrap_err("failed to add input ID")
                    .unwrap();
                "INPUT_CLOSED"
            }
        };

        dict.set_item("type", ty)
            .wrap_err("could not make type a python dictionary item")
            .unwrap();

        dict.into()
    }
}

#[derive(Debug)]
pub enum StopReason {
    InputsClosed,
    ExplicitStop,
    ExplicitStopAll,
}
