//! EthFlow deployment metadata.
//!
//! EthFlow lets users sell a chain's native gas token through CoW
//! Protocol by placing an on-chain order against a per-chain
//! `CoWSwapEthFlow` contract. Unlike the settlement and vault relayer
//! singletons, callers should treat EthFlow as a per-chain deployment
//! table: the current upstream artifacts happen to share the same
//! prod/barn contract addresses across the crate-supported chains, but the
//! lookup is still kept per-chain so future deployments can diverge without
//! changing call sites.

use alloy_primitives::{Address, address};

use crate::chain::Chain;

/// Current production EthFlow contract address in the upstream
/// `ethflowcontract` deployment artifacts.
pub const ETH_FLOW_PRODUCTION: Address = address!("bA3cB449bD2B4ADddBc894D8697F5170800EAdeC");

/// Current barn/staging EthFlow contract address in the upstream
/// `ethflowcontract` deployment artifacts.
pub const ETH_FLOW_STAGING: Address = address!("04501b9b1D52e67f6862d157E00D13419D2D6E95");

/// EthFlow deployment environment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EthFlowEnvironment {
    /// Production deployment used with `https://api.cow.fi`.
    Production,
    /// Barn/staging deployment used with `https://barn.api.cow.fi`.
    Barn,
}

/// Contract address and wrapped-native token for one EthFlow deployment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EthFlowDeployment {
    /// `CoWSwapEthFlow` contract address.
    pub contract: Address,
    /// Wrapped native token passed to the EthFlow constructor.
    pub wrapped_native_token: Address,
}

impl Chain {
    /// Production EthFlow deployment metadata for this chain.
    pub const fn eth_flow_deployment(self) -> EthFlowDeployment {
        EthFlowDeployment {
            contract: self.eth_flow_contract(EthFlowEnvironment::Production),
            wrapped_native_token: self.wrapped_native_token(),
        }
    }

    /// Barn/staging EthFlow deployment metadata for this chain.
    ///
    /// The deployment artifacts currently publish barn contracts for every
    /// crate-supported chain. This is independent from
    /// [`Chain::orderbook_barn_url`], which reports where the hosted barn
    /// API is available.
    pub const fn eth_flow_barn_deployment(self) -> EthFlowDeployment {
        EthFlowDeployment {
            contract: self.eth_flow_contract(EthFlowEnvironment::Barn),
            wrapped_native_token: self.wrapped_native_token(),
        }
    }

    /// EthFlow deployment metadata for `environment`.
    pub const fn eth_flow_deployment_for(
        self,
        environment: EthFlowEnvironment,
    ) -> EthFlowDeployment {
        match environment {
            EthFlowEnvironment::Production => self.eth_flow_deployment(),
            EthFlowEnvironment::Barn => self.eth_flow_barn_deployment(),
        }
    }

    /// Production EthFlow contract address for this chain.
    pub const fn eth_flow_address(self) -> Address {
        self.eth_flow_deployment().contract
    }

    /// Barn/staging EthFlow contract address for this chain.
    pub const fn eth_flow_barn_address(self) -> Address {
        self.eth_flow_barn_deployment().contract
    }

    const fn eth_flow_contract(self, environment: EthFlowEnvironment) -> Address {
        let (production, barn) = match self {
            Self::Mainnet
            | Self::Bnb
            | Self::Gnosis
            | Self::Polygon
            | Self::Base
            | Self::Plasma
            | Self::ArbitrumOne
            | Self::Avalanche
            | Self::Linea
            | Self::Sepolia => (ETH_FLOW_PRODUCTION, ETH_FLOW_STAGING),
        };
        match environment {
            EthFlowEnvironment::Production => production,
            EthFlowEnvironment::Barn => barn,
        }
    }

    /// Wrapped native token used by EthFlow on this chain.
    pub const fn wrapped_native_token(self) -> Address {
        match self {
            Self::Mainnet => address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            Self::Bnb => address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"),
            Self::Gnosis => address!("e91D153E0b41518A2Ce8Dd3D7944Fa863463a97d"),
            Self::Polygon => address!("0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270"),
            Self::Base => address!("4200000000000000000000000000000000000006"),
            Self::Plasma => address!("6100E367285b01F48D07953803A2d8dCA5D19873"),
            Self::ArbitrumOne => address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            Self::Avalanche => address!("B31f66AA3C1e785363F0875A1B74E27b85FD66c7"),
            Self::Linea => address!("e5D7C2a44FfDDf6b295A15c148167daaAf5Cf34f"),
            Self::Sepolia => address!("fFf9976782d46CC05630D1f6eBAb18b2324d6B14"),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use crate::Chain;

    #[test]
    fn eth_flow_deployments_match_upstream_artifacts() {
        let cases = [
            (
                Chain::Mainnet,
                address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            ),
            (
                Chain::Bnb,
                address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"),
            ),
            (
                Chain::Gnosis,
                address!("e91D153E0b41518A2Ce8Dd3D7944Fa863463a97d"),
            ),
            (
                Chain::Polygon,
                address!("0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270"),
            ),
            (
                Chain::Base,
                address!("4200000000000000000000000000000000000006"),
            ),
            (
                Chain::Plasma,
                address!("6100E367285b01F48D07953803A2d8dCA5D19873"),
            ),
            (
                Chain::ArbitrumOne,
                address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            ),
            (
                Chain::Avalanche,
                address!("B31f66AA3C1e785363F0875A1B74E27b85FD66c7"),
            ),
            (
                Chain::Linea,
                address!("e5D7C2a44FfDDf6b295A15c148167daaAf5Cf34f"),
            ),
            (
                Chain::Sepolia,
                address!("fFf9976782d46CC05630D1f6eBAb18b2324d6B14"),
            ),
        ];

        for (chain, wrapped_native_token) in cases {
            assert_eq!(chain.eth_flow_address(), super::ETH_FLOW_PRODUCTION);
            assert_eq!(chain.eth_flow_barn_address(), super::ETH_FLOW_STAGING);
            assert_eq!(
                chain.eth_flow_deployment().wrapped_native_token,
                wrapped_native_token
            );
            assert_eq!(chain.wrapped_native_token(), wrapped_native_token);
        }
    }

    #[test]
    fn eth_flow_environment_selects_prod_or_barn() {
        let chain = Chain::Sepolia;

        assert_eq!(
            chain.eth_flow_deployment_for(super::EthFlowEnvironment::Production),
            chain.eth_flow_deployment()
        );
        assert_eq!(
            chain.eth_flow_deployment_for(super::EthFlowEnvironment::Barn),
            chain.eth_flow_barn_deployment()
        );
    }
}
