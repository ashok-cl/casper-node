//! Block validator
//!
//! The block validator checks whether all the deploys included in the proto block exist, either
//! locally or in the network

use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    mem,
};

use derive_more::{Display, From};
use rand::Rng;
use tracing::warn;

use crate::{
    components::Component,
    effect::{
        requests::{BlockValidatorRequest, FetcherRequest},
        EffectBuilder, EffectExt, Effects, Responder,
    },
    types::{Deploy, DeployHash, ProtoBlock},
};

/// Block validator component event.
#[derive(Debug, From, Display)]
pub enum Event<I> {
    /// A request made of the Block validator component.
    #[from]
    Request(BlockValidatorRequest<I>),
    #[display(fmt = "deploy {} is valid: {}", deploy_hash, downloaded_successfully)]
    DeployFetcherResponse {
        deploy_hash: DeployHash,
        downloaded_successfully: bool,
    },
}

#[derive(Debug)]
struct BlockData {
    missing_deploys: HashSet<DeployHash>,
    responder: Responder<(bool, ProtoBlock)>,
}

/// Block validator.
#[derive(Debug, Default)]
pub(crate) struct BlockValidator<I> {
    block_states: HashMap<ProtoBlock, BlockData>,
    _marker: std::marker::PhantomData<I>,
}

impl<I> BlockValidator<I> {
    pub(crate) fn new() -> Self {
        BlockValidator {
            block_states: Default::default(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<I, REv> Component<REv> for BlockValidator<I>
where
    I: Clone + Send + 'static,
    REv: From<Event<I>> + From<BlockValidatorRequest<I>> + From<FetcherRequest<I, Deploy>> + Send,
{
    type Event = Event<I>;

    fn handle_event<R: Rng + ?Sized>(
        &mut self,
        effect_builder: EffectBuilder<REv>,
        _rng: &mut R,
        event: Self::Event,
    ) -> Effects<Self::Event> {
        match event {
            Event::Request(BlockValidatorRequest {
                proto_block,
                sender,
                responder,
            }) => {
                let missing_deploys: HashSet<_> = proto_block.deploys.iter().cloned().collect();
                self.block_states.insert(
                    proto_block,
                    BlockData {
                        missing_deploys: missing_deploys.clone(),
                        responder,
                    },
                );
                missing_deploys
                    .into_iter()
                    .flat_map(|deploy_hash| {
                        effect_builder
                            .fetch_deploy(deploy_hash, sender.clone())
                            .event(move |maybe_deploy| Event::DeployFetcherResponse {
                                deploy_hash,
                                downloaded_successfully: maybe_deploy.is_some(),
                            })
                    })
                    .collect()
            }
            Event::DeployFetcherResponse {
                deploy_hash,
                downloaded_successfully,
            } => {
                let block_states = mem::take(&mut self.block_states);
                let mut effects: Effects<Self::Event> = Default::default();
                if downloaded_successfully {
                    for (proto_block, mut block_data) in block_states {
                        block_data.missing_deploys.remove(&deploy_hash);
                        if block_data.missing_deploys.is_empty() {
                            effects.extend(
                                block_data
                                    .responder
                                    .respond((true, proto_block))
                                    .ignore::<Event<I>>(),
                            );
                        } else {
                            self.block_states.insert(proto_block, block_data);
                        }
                    }
                } else {
                    for (proto_block, block_data) in block_states {
                        if block_data.missing_deploys.contains(&deploy_hash) {
                            warn!(
                                "{} considered invalid due to inability to download {}",
                                proto_block, deploy_hash
                            );
                            effects.extend(
                                block_data
                                    .responder
                                    .respond((false, proto_block))
                                    .ignore::<Event<I>>(),
                            );
                        } else {
                            self.block_states.insert(proto_block, block_data);
                        }
                    }
                }
                effects
            }
        }
    }
}