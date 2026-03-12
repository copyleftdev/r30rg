use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, RootProvider};
use r30rg_core::types::{Layer, LayerEndpoint};

/// RPC harness — connects to all layers and provides adversarial primitives.
/// Uses RootProvider directly (no fillers) since we only need read operations
/// and raw-level control for adversarial testing.
pub struct RpcHarness {
    pub l1: LayerConnection,
    pub l2: LayerConnection,
    pub l3: Option<LayerConnection>,
}

pub struct LayerConnection {
    pub layer: Layer,
    pub provider: RootProvider,
    pub chain_id: u64,
    pub rpc_url: String,
}

impl RpcHarness {
    pub async fn connect(
        l1: &LayerEndpoint,
        l2: &LayerEndpoint,
        l3: Option<&LayerEndpoint>,
    ) -> anyhow::Result<Self> {
        let l1_conn = Self::connect_layer(l1).await?;
        let l2_conn = Self::connect_layer(l2).await?;
        let l3_conn = match l3 {
            Some(ep) => Some(Self::connect_layer(ep).await?),
            None => None,
        };
        Ok(Self {
            l1: l1_conn,
            l2: l2_conn,
            l3: l3_conn,
        })
    }

    async fn connect_layer(ep: &LayerEndpoint) -> anyhow::Result<LayerConnection> {
        let url: url::Url = ep.rpc_url.parse()?;
        let provider = RootProvider::new_http(url);
        let chain_id = provider.get_chain_id().await?;
        tracing::info!(
            layer = %ep.layer,
            chain_id = chain_id,
            url = %ep.rpc_url,
            "connected"
        );
        Ok(LayerConnection {
            layer: ep.layer,
            provider,
            chain_id,
            rpc_url: ep.rpc_url.clone(),
        })
    }

    /// Get block number on a given layer.
    pub async fn block_number(&self, layer: Layer) -> anyhow::Result<u64> {
        let conn = self.get_layer(layer)?;
        Ok(conn.provider.get_block_number().await?)
    }

    /// Get balance of an address on a given layer.
    pub async fn balance(&self, layer: Layer, addr: Address) -> anyhow::Result<U256> {
        let conn = self.get_layer(layer)?;
        Ok(conn.provider.get_balance(addr).await?)
    }

    /// Get gas price on a given layer.
    pub async fn gas_price(&self, layer: Layer) -> anyhow::Result<u128> {
        let conn = self.get_layer(layer)?;
        Ok(conn.provider.get_gas_price().await?)
    }

    /// Call an arbitrary contract (read-only).
    pub async fn eth_call(
        &self,
        layer: Layer,
        to: Address,
        data: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        let conn = self.get_layer(layer)?;
        let tx = alloy::rpc::types::TransactionRequest::default()
            .to(to)
            .input(alloy::rpc::types::TransactionInput::new(Bytes::from(data)));
        let result = conn.provider.call(tx).await?;
        Ok(result.to_vec())
    }

    fn get_layer(&self, layer: Layer) -> anyhow::Result<&LayerConnection> {
        match layer {
            Layer::L1 => Ok(&self.l1),
            Layer::L2 => Ok(&self.l2),
            Layer::L3 => self
                .l3
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("L3 not configured")),
        }
    }
}
