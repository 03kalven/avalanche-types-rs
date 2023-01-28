#![allow(deprecated)]

use std::{convert::TryFrom, env::args, io, str::FromStr};

use avalanche_types::{
    evm::{abi, eip712::gsn::Tx},
    jsonrpc::client::evm as json_client_evm,
    key,
};
use ethers::prelude::Eip1559TransactionRequest;
use ethers_core::{
    abi::{Function, StateMutability},
    types::transaction::eip2718::TypedTransaction,
    types::{H160, U256},
};
use ethers_providers::{Http, Middleware, Provider};

/// cargo run --example evm_contract_counter_increment_forwarder_relay_eip712 --features="jsonrpc_client evm" -- [RELAY SERVER HTTP RPC ENDPOINT] [EVM HTTP RPC ENDPOINT] [FORWARDER CONTRACT ADDRESS] [RECIPIENT CONTRACT ADDRESS]
/// cargo run --example evm_contract_counter_increment_forwarder_relay_eip712 --features="jsonrpc_client evm" -- http://127.0.0.1:9876/rpc http://127.0.0.1:9650/ext/bc/C/rpc 0x52C84043CD9c865236f11d9Fc9F56aa003c1f922 0x5DB9A7629912EBF95876228C24A848de0bfB43A9
#[tokio::main]
async fn main() -> io::Result<()> {
    // ref. https://github.com/env-logger-rs/env_logger/issues/47
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let relay_server_rpc_url = args().nth(1).expect("no relay server RPC URL given");
    let relay_server_provider = Provider::<Http>::try_from(relay_server_rpc_url.clone())
        .expect("could not instantiate HTTP Provider");
    log::info!("created relay server provider for {relay_server_rpc_url}");

    let chain_rpc_url = args().nth(2).expect("no chain RPC URL given");
    let chain_rpc_provider = Provider::<Http>::try_from(chain_rpc_url.clone())
        .expect("could not instantiate HTTP Provider");
    log::info!("created chain rpc server provider for {chain_rpc_url}");

    let forwarder_contract_addr = args().nth(3).expect("no forwarder contract address given");
    let forwarder_contract_addr =
        H160::from_str(forwarder_contract_addr.trim_start_matches("0x")).unwrap();

    let recipient_contract_addr = args().nth(4).expect("no recipient contract address given");
    let recipient_contract_addr =
        H160::from_str(recipient_contract_addr.trim_start_matches("0x")).unwrap();

    let chain_id = json_client_evm::chain_id(&chain_rpc_url).await.unwrap();
    log::info!(
        "running against {chain_rpc_url}, {chain_id} for forwarder contract {forwarder_contract_addr}, recipient contract {recipient_contract_addr}"
    );

    let no_gas_key = key::secp256k1::private_key::Key::generate().unwrap();
    let no_gas_key_info = no_gas_key.to_info(1).unwrap();
    log::info!("created hot key:\n\n{}\n", no_gas_key_info);
    let no_gas_key_signer: ethers_signers::LocalWallet =
        no_gas_key.to_ethers_core_signing_key().into();

    // parsed function of "increment()"
    let func = Function {
        name: "increment".to_string(),
        inputs: vec![],
        outputs: Vec::new(),
        constant: None,
        state_mutability: StateMutability::NonPayable,
    };
    let arg_tokens = vec![];
    let no_gas_recipient_contract_calldata = abi::encode_calldata(func, &arg_tokens).unwrap();
    log::info!(
        "no gas recipient contract calldata: 0x{}",
        hex::encode(no_gas_recipient_contract_calldata.clone())
    );

    let relay_tx = Tx::new()
        //
        // make sure this matches with "registerDomainSeparator" call
        .domain_name("my name")
        .domain_version("1")
        //
        // local network
        .domain_chain_id(chain_id)
        //
        // trusted forwarder contract address
        .domain_verifying_contract(forwarder_contract_addr)
        .from(no_gas_key_info.h160_address.clone())
        //
        // contract address that this gasless transaction will interact with
        .to(recipient_contract_addr)
        //
        // fails if zero (e.g., "out of gas")
        // TODO: better estimate gas based on "RelayHub", use "eth_estimateGas"
        .gas(U256::from(30000))
        //
        // contract call needs no value
        .value(U256::zero())
        //
        // assume this is the first transaction
        .nonce(U256::from(0))
        //
        // calldata for contract calls
        .data(no_gas_recipient_contract_calldata)
        //
        //
        .valid_until_time(U256::MAX)
        //
        //
        .type_name("my name")
        //
        //
        .type_suffix_data("my suffix");
    let relay_tx_request = relay_tx
        .sign_to_request(no_gas_key_signer.clone())
        .await
        .unwrap();
    let signed_bytes: ethers_core::types::Bytes =
        serde_json::to_vec(&relay_tx_request).unwrap().into();
    log::info!("relay_tx_request: {:?}", relay_tx_request);

    let relay_tx_calldata = relay_tx
        .encode_execute_call(signed_bytes.to_vec().clone())
        .unwrap();
    let eip1559_tx = Eip1559TransactionRequest::new()
        .chain_id(chain_id.as_u64())
        .to(ethers::prelude::H160::from(
            forwarder_contract_addr.as_fixed_bytes(),
        ))
        .data(relay_tx_calldata);
    let typed_tx: TypedTransaction = eip1559_tx.into();
    let estimated_gas = chain_rpc_provider
        .estimate_gas(&typed_tx, None)
        .await
        .unwrap();
    log::info!("estimated gas: {estimated_gas}");

    let relay_tx = relay_tx.gas(estimated_gas.checked_add(U256::from(5000)).unwrap());
    let relay_tx_request = relay_tx.sign_to_request(no_gas_key_signer).await.unwrap();
    let signed_bytes: ethers_core::types::Bytes =
        serde_json::to_vec(&relay_tx_request).unwrap().into();
    log::info!("relay_tx_request: {:?}", relay_tx_request);

    let pending = relay_server_provider
        .send_raw_transaction(signed_bytes)
        .await
        .unwrap();
    log::info!(
        "pending tx hash {} from 0x{:x}",
        pending.tx_hash(),
        no_gas_key_info.h160_address
    );

    Ok(())
}
