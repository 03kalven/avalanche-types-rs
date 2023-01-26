#![allow(deprecated)]

use std::{env::args, io, str::FromStr};

use avalanche_types::{
    evm::{abi, eip712::gsn::Tx},
    jsonrpc::client::evm as json_client_evm,
    key, wallet,
};
use ethers_core::{
    abi::{Function, StateMutability},
    types::{H160, U256},
};

/// Sends a request to the forwarder.
///
/// cargo run --example evm_contract_forwarder_execute_counter_increment --features="jsonrpc_client evm" -- [HTTP RPC ENDPOINT] [GAS PAYER PRIVATE KEY] [FORWARDER CONTRACT ADDRESS] [RECIPIENT CONTRACT ADDRESS]
/// cargo run --example evm_contract_forwarder_execute_counter_increment --features="jsonrpc_client evm" -- http://127.0.0.1:9650/ext/bc/C/rpc 56289e99c94b6912bfc12adc093c9b51124f0dc54ac7a766b2bc5ccf558d8027 0x7466154c5DE2680Ee2767C763546F052DC7bC393 0x59289F9Ea2432226c8430e3057E2642aD5f979aE
///
/// cast send --gas-price 700000000000 --priority-gas-price 10000000000 --private-key=56289e99c94b6912bfc12adc093c9b51124f0dc54ac7a766b2bc5ccf558d8027 --rpc-url=http://127.0.0.1:9650/ext/bc/C/rpc 0x59289F9Ea2432226c8430e3057E2642aD5f979aE "increment()"
/// cast call --rpc-url=http://127.0.0.1:9650/ext/bc/C/rpc 0x59289F9Ea2432226c8430e3057E2642aD5f979aE "getNumber()" | sed -r '/^\s*$/d' | tail -1
/// cast call --rpc-url=http://127.0.0.1:9650/ext/bc/C/rpc 0x59289F9Ea2432226c8430e3057E2642aD5f979aE "getLast()"
///
/// cast receipt --rpc-url=http://127.0.0.1:9650/ext/bc/C/rpc 0x31b977eff419b20c7f0e1c612530258e65cf51a38676b4c7930060ec3b9f10ee
#[tokio::main]
async fn main() -> io::Result<()> {
    // ref. https://github.com/env-logger-rs/env_logger/issues/47
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let chain_rpc_url = args().nth(1).expect("no url given");
    let private_key = args().nth(2).expect("no private key given");

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
    let no_gas_signer: ethers_signers::LocalWallet = no_gas_key.to_ethers_core_signing_key().into();

    // parsed function of "increment()"
    let func = Function {
        name: "increment".to_string(),
        inputs: vec![],
        outputs: Vec::new(),
        constant: None,
        state_mutability: StateMutability::NonPayable,
    };
    let arg_tokens = vec![];
    let no_gas_calldata = abi::encode_calldata(func, &arg_tokens).unwrap();
    log::info!(
        "no gas calldata: 0x{}",
        hex::encode(no_gas_calldata.clone())
    );

    let rr_tx = Tx::new()
        //
        // make sure this matches with "registerDomainSeparator" call
        .domain_name("my name")
        //
        .domain_version("1")
        //
        // local network
        .domain_chain_id(U256::from(43112))
        //
        // trusted forwarder contract address
        .domain_verifying_contract(forwarder_contract_addr)
        //
        // gasless transaction signer
        .from(no_gas_key_info.h160_address.clone())
        //
        // contract address that this gasless transaction will interact with
        .to(recipient_contract_addr)
        //
        // contract call needs no value
        .value(U256::zero())
        //
        // initial nonce is zero
        .nonce(U256::from(0))
        //
        .data(no_gas_calldata)
        //
        .valid_until_time(U256::MAX)
        //
        .type_name("my name")
        //
        .type_suffix_data("my suffix");

    let no_gas_sig = rr_tx.sign(no_gas_signer.clone()).await.unwrap();
    log::info!("gas payer sig: 0x{}", hex::encode(no_gas_sig.clone()));

    let gas_payer_calldata = rr_tx.encode_execute_call(no_gas_sig).unwrap();
    log::info!(
        "gas payer calldata: 0x{}",
        hex::encode(gas_payer_calldata.clone())
    );

    let gas_payer_key = key::secp256k1::private_key::Key::from_hex(private_key).unwrap();
    let gas_payer_key_info = gas_payer_key.to_info(1).unwrap();
    log::info!("created hot key:\n\n{}\n", gas_payer_key_info);
    let gas_payer_signer: ethers_signers::LocalWallet =
        gas_payer_key.to_ethers_core_signing_key().into();
    let w = wallet::Builder::new(&gas_payer_key)
        .base_http_url(chain_rpc_url.clone())
        .build()
        .await?;
    let gas_payer_evm_wallet = w.evm(
        &gas_payer_signer,
        chain_rpc_url.as_str(),
        U256::from(chain_id),
    )?;

    let tx_id = gas_payer_evm_wallet
        .eip1559()
        .recipient(forwarder_contract_addr)
        .data(gas_payer_calldata)
        .urgent()
        .check_acceptance(true)
        .submit()
        .await?;
    log::info!("evm ethers wallet SUCCESS with transaction id {}", tx_id);

    Ok(())
}
