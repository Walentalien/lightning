use std::collections::BTreeMap;
use std::time::Duration;

use fleek_crypto::{AccountOwnerSecretKey, NodeSecretKey, SecretKey};
use hp_fixed::unsigned::HpUfixed;
use lightning_committee_beacon::CommitteeBeaconConfig;
use lightning_interfaces::types::{
    CommitteeSelectionBeaconPhase,
    DeliveryAcknowledgmentProof,
    ExecutionData,
    ExecutionError,
    TransactionReceipt,
    TransactionResponse,
    UpdateMethod,
};
use lightning_interfaces::SyncQueryRunnerInterface;
use lightning_test_utils::consensus::MockConsensusConfig;
use lightning_test_utils::e2e::{
    DowncastToTestFullNode,
    TestFullNodeComponentsWithMockConsensus,
    TestNetwork,
};
use lightning_utils::application::QueryRunnerExt;
use lightning_utils::poll::{poll_until, PollUntilError};
use tempfile::tempdir;
use utils::{
    create_genesis_committee,
    deposit_and_stake,
    expect_tx_revert,
    expect_tx_success,
    prepare_update_request_account,
    prepare_update_request_node,
    test_init_app,
    test_reputation_measurements,
};

use super::*;

#[tokio::test]
async fn test_epoch_change_with_all_committee_nodes() {
    let mut network = TestNetwork::builder()
        .with_mock_consensus(MockConsensusConfig {
            block_buffering_interval: Duration::from_millis(100),
            max_ordering_time: 1,
            ..Default::default()
        })
        .with_committee_beacon_config(CommitteeBeaconConfig::default())
        .with_genesis_mutator(|genesis| {
            genesis.committee_selection_beacon_commit_phase_duration = 3;
            genesis.committee_selection_beacon_reveal_phase_duration = 3;
        })
        .with_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(4)
        .await
        .build()
        .await
        .unwrap();
    let node1 = network.node(0);
    let node2 = network.node(1);
    let node3 = network.node(2);

    // Get the current epoch.
    let epoch = node1.app_query().get_current_epoch();

    // Execute an epoch change transaction from less than 2/3 of the nodes.
    node1
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();
    node2
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();

    // Check that the epoch has not been changed within some time period.
    let result = poll_until(
        || async {
            network
                .nodes()
                .all(|node| node.app_query().get_current_epoch() != epoch)
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(1),
        Duration::from_millis(100),
    )
    .await;
    assert_eq!(result.unwrap_err(), PollUntilError::Timeout);

    // Check that the ready-to-change set in the committee info contains the nodes that sent an
    // epoch change transaction.
    poll_until(
        || async {
            network
                .nodes()
                .all(|node| {
                    node.app_query()
                        .get_committee_info(&epoch, |c| c.ready_to_change)
                        .unwrap()
                        == vec![0, 1]
                })
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(3),
        Duration::from_millis(100),
    )
    .await
    .unwrap();

    // Execute an epoch change transaction from enough nodes to trigger an epoch change.
    node3
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();

    // Wait for commit phase to start across all nodes, even the one that did not send an epoch
    // change transaction.
    poll_until(
        || async {
            network
                .nodes()
                .all(|node| {
                    node.app_query().get_committee_selection_beacon_phase()
                        == Some(CommitteeSelectionBeaconPhase::Commit((0, 0)))
                })
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(20),
        Duration::from_millis(100),
    )
    .await
    .unwrap();

    // Check that the ready-to-change set in the committee info contains all the nodes that sent an
    // epoch change transaction.
    for node in network.nodes() {
        assert_eq!(
            node.app_query()
                .get_committee_info(&epoch, |c| c.ready_to_change)
                .unwrap(),
            vec![0, 1, 2]
        );
    }

    // Check that the ready-to-change set for the next epoch is empty.
    for node in network.nodes() {
        assert!(node
            .app_query()
            .get_committee_info(&(epoch + 1), |c| c.ready_to_change)
            .unwrap_or_default()
            .is_empty());
    }

    // Shutdown the network.
    network.shutdown().await;
}

#[tokio::test]
async fn test_epoch_change_with_some_non_committee_nodes() {
    let commit_phase_duration = 2000;
    let reveal_phase_duration = 2000;
    let mut network = TestNetwork::builder()
        .with_mock_consensus(MockConsensusConfig {
            block_buffering_interval: Duration::from_millis(100),
            max_ordering_time: 1,
            ..Default::default()
        })
        .with_committee_beacon_config(CommitteeBeaconConfig::default())
        .with_genesis_mutator(move |genesis| {
            genesis.committee_selection_beacon_commit_phase_duration = commit_phase_duration;
            genesis.committee_selection_beacon_reveal_phase_duration = reveal_phase_duration;
        })
        .with_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(4)
        .await
        .with_non_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(2)
        .await
        .build()
        .await
        .unwrap();

    // Get the current committee nodes.
    let committee_nodes = network.committee_nodes();
    let committee_node1 = committee_nodes[0];
    let committee_node2 = committee_nodes[1];
    let committee_node3 = committee_nodes[2];

    // Get the current non-committee nodes.
    let non_committee_nodes = network.non_committee_nodes();
    let non_committee_node1 = non_committee_nodes[0];
    let non_committee_node2 = non_committee_nodes[1];

    // Get the current epoch.
    let epoch = network.node(0).app_query().get_current_epoch();

    // Execute an epoch change transaction from less than 2/3 of the committee nodes.
    committee_node1
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();
    committee_node2
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();

    // Check that the ready-to-change set in the committee info contains the nodes that sent an
    // epoch change transaction.
    poll_until(
        || async {
            network
                .nodes()
                .all(|node| {
                    node.app_query()
                        .get_committee_info(&epoch, |c| c.ready_to_change)
                        .unwrap()
                        == vec![0, 1]
                })
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(3),
        Duration::from_millis(100),
    )
    .await
    .unwrap();

    // Send epoch change transactions from the non-committee nodes.
    let receipt = non_committee_node1
        .execute_transaction_from_node_with_receipt(
            UpdateMethod::ChangeEpoch { epoch },
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert!(matches!(
        receipt,
        TransactionReceipt {
            response: TransactionResponse::Revert(ExecutionError::NotCommitteeMember),
            ..
        },
    ));

    let receipt = non_committee_node2
        .execute_transaction_from_node_with_receipt(
            UpdateMethod::ChangeEpoch { epoch },
            Duration::from_secs(10),
        )
        .await
        .unwrap();

    assert!(matches!(
        receipt,
        TransactionReceipt {
            response: TransactionResponse::Revert(ExecutionError::NotCommitteeMember),
            ..
        },
    ));

    // Check that the commit phase has not started within some time period.
    poll_until(
        || async {
            network
                .nodes()
                .all(|node| {
                    node.app_query()
                        .get_committee_selection_beacon_phase()
                        .is_none()
                })
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(1),
        Duration::from_millis(100),
    )
    .await
    .unwrap();

    // Check that the ready-to-change set in the committee info contains the nodes that sent an
    // epoch change transaction.
    for node in network.nodes() {
        assert_eq!(
            node.app_query()
                .get_committee_info(&epoch, |c| c.ready_to_change)
                .unwrap(),
            vec![0, 1]
        );
    }

    // Execute an epoch change transaction from enough nodes to trigger an epoch change.
    committee_node3
        .execute_transaction_from_node(UpdateMethod::ChangeEpoch { epoch })
        .await
        .unwrap();

    // Check that the commit phase has started.
    poll_until(
        || async {
            network
                .nodes()
                .all(|node| {
                    node.app_query().get_committee_selection_beacon_phase()
                        == Some(CommitteeSelectionBeaconPhase::Commit((0, 0)))
                })
                .then_some(())
                .ok_or(PollUntilError::ConditionNotSatisfied)
        },
        Duration::from_secs(1),
        Duration::from_millis(100),
    )
    .await
    .unwrap();

    // Wait for the commit phase to end.
    tokio::time::sleep(Duration::from_millis(commit_phase_duration)).await;
    // Send the commit phase timeout transaction from 2/3+1 committee nodes.
    network.commit_phase_timeout(0).await.unwrap();

    // Wait for the reveal phase to end.
    tokio::time::sleep(Duration::from_millis(reveal_phase_duration)).await;
    // Send the reveal phase timeout transaction from 2/3+1 committee nodes.
    network.reveal_phase_timeout(0).await.unwrap();

    // Wait for epoch to be incremented across all nodes, even the one that did not send an epoch
    // change transaction.
    network.wait_for_epoch_change(epoch + 1).await.unwrap();

    // Check that the ready-to-change set in the committee info contains all the nodes that sent an
    // epoch change transaction.
    for node in network.nodes() {
        assert_eq!(
            node.app_query()
                .get_committee_info(&epoch, |c| c.ready_to_change)
                .unwrap(),
            vec![0, 1, 2]
        );
    }

    // Check that the ready-to-change set for the next epoch is empty.
    for node in network.nodes() {
        assert!(node
            .app_query()
            .get_committee_info(&(epoch + 1), |c| c.ready_to_change)
            .unwrap_or_default()
            .is_empty());
    }

    // Shutdown the network.
    network.shutdown().await;
}

#[tokio::test]
async fn test_change_epoch_with_only_locked_stake() {
    let network = utils::TestNetwork::builder()
        .with_committee_nodes(2)
        .with_genesis_mutator(|genesis| {
            genesis.min_stake = 1000;

            // First node has only locked stake.
            genesis.node_info[0].stake.staked = 0u32.into();
            genesis.node_info[0].stake.locked = 1000u32.into();

            // Second node has unlocked stake.
            genesis.node_info[1].stake.staked = 1000u32.into();
            genesis.node_info[1].stake.locked = 0u32.into();
        })
        .build()
        .await
        .unwrap();
    let query = network.query();
    let epoch = query.get_current_epoch();

    // Execute epoch change transaction from the node with only locked stake.
    let resp = network
        .execute(vec![network
            .node(0)
            .build_transaction(UpdateMethod::ChangeEpoch { epoch })])
        .await
        .unwrap();
    assert_eq!(resp.block_number, 1);
    assert!(!resp.change_epoch);

    // Execute epoch change transaction from the node with unlocked stake.
    let resp = network
        .execute(vec![network
            .node(1)
            .build_transaction(UpdateMethod::ChangeEpoch { epoch })])
        .await
        .unwrap();
    assert_eq!(resp.block_number, 2);
    assert!(!resp.change_epoch);

    // Check that we have transitioned to the committee beacon commit phase.
    assert_eq!(
        query.get_committee_selection_beacon_phase(),
        Some(CommitteeSelectionBeaconPhase::Commit((0, 0)))
    );
}

#[tokio::test]
async fn test_change_epoch_if_node_opted_out() {
    let network = utils::TestNetwork::builder()
        .with_committee_nodes(2)
        .build()
        .await
        .unwrap();
    let query = network.query();
    let epoch = query.get_current_epoch();

    // Execute opt-out transaction from the first node.
    let resp = network
        .execute(vec![network
            .node(0)
            .build_transaction(UpdateMethod::OptOut {})])
        .await
        .unwrap();
    assert_eq!(resp.block_number, 1);

    // Execute epoch change transaction from the node and check that it reverts.
    let resp = network
        .maybe_execute(vec![network
            .node(0)
            .build_transaction(UpdateMethod::ChangeEpoch { epoch })])
        .await
        .unwrap();
    assert_eq!(resp.block_number, 2);
    assert!(!resp.change_epoch);
    assert_eq!(
        resp.txn_receipts[0].response,
        TransactionResponse::Success(ExecutionData::None)
    );
}

#[tokio::test]
async fn test_change_epoch_reverts_account_key() {
    let temp_dir = tempdir().unwrap();

    let committee_size = 4;
    let (committee, _keystore) = create_genesis_committee(committee_size);
    let (update_socket, _query_runner) = test_init_app(&temp_dir, committee);

    // Account Secret Key
    let secret_key = AccountOwnerSecretKey::generate();

    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 0 };

    let update = prepare_update_request_account(change_epoch, &secret_key, 1);
    expect_tx_revert(update, &update_socket, ExecutionError::OnlyNode).await;
}

#[tokio::test]
async fn test_change_epoch_reverts_node_does_not_exist() {
    let temp_dir = tempdir().unwrap();

    // Create a genesis committee and seed the application state with it.
    let committee_size = 4;
    let (committee, _keystore) = create_genesis_committee(committee_size);
    let (update_socket, _query_runner) = test_init_app(&temp_dir, committee);

    // Unknown Node Key (without Stake)
    let node_secret_key = NodeSecretKey::generate();
    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 0 };

    let update = prepare_update_request_node(change_epoch, &node_secret_key, 1);
    expect_tx_revert(update, &update_socket, ExecutionError::NodeDoesNotExist).await;
}

#[tokio::test]
async fn test_change_epoch_reverts_insufficient_stake() {
    let temp_dir = tempdir().unwrap();

    // Create a genesis committee and seed the application state with it.
    let committee_size = 4;
    let (committee, _keystore) = create_genesis_committee(committee_size);
    let (update_socket, query_runner) = test_init_app(&temp_dir, committee);

    let owner_secret_key = AccountOwnerSecretKey::generate();
    // New Node key
    let node_secret_key = NodeSecretKey::generate();

    // Stake less than the minimum required amount.
    let minimum_stake_amount: HpUfixed<18> = query_runner.get_staking_amount().into();
    let less_than_minimum_stake_amount: HpUfixed<18> =
        minimum_stake_amount / HpUfixed::<18>::from(2u16);
    deposit_and_stake(
        &update_socket,
        &owner_secret_key,
        1,
        &less_than_minimum_stake_amount,
        &node_secret_key.to_pk(),
        [0; 96].into(),
    )
    .await;

    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 0 };
    let update = prepare_update_request_node(change_epoch, &node_secret_key, 1);
    expect_tx_revert(update, &update_socket, ExecutionError::InsufficientStake).await;
}

#[tokio::test]
async fn test_epoch_change_reverts_epoch_already_changed() {
    let commit_phase_duration = 2000;
    let reveal_phase_duration = 2000;
    let mut network = TestNetwork::builder()
        .with_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(4)
        .await
        .build()
        .await
        .unwrap();
    let node = network.node(0);
    let epoch = node.app_query().get_current_epoch();

    network
        .change_epoch_and_wait_for_complete(0, commit_phase_duration, reveal_phase_duration)
        .await
        .unwrap();

    // Send epoch change transaction from a node for same epoch, and expect it to be reverted.
    let receipt = node
        .execute_transaction_from_node_with_receipt(
            UpdateMethod::ChangeEpoch { epoch },
            Duration::from_secs(10),
        )
        .await
        .unwrap();

    assert!(matches!(
        receipt,
        TransactionReceipt {
            response: TransactionResponse::Revert(ExecutionError::EpochAlreadyChanged),
            ..
        },
    ));

    // Shutdown the network.
    network.shutdown().await;
}

#[tokio::test]
async fn test_epoch_change_reverts_epoch_has_not_started() {
    let temp_dir = tempdir().unwrap();

    // Create a genesis committee and seed the application state with it.
    let committee_size = 4;
    let (committee, keystore) = create_genesis_committee(committee_size);
    let (update_socket, _query_runner) = test_init_app(&temp_dir, committee);

    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 1 };
    let update = prepare_update_request_node(change_epoch, &keystore[0].node_secret_key, 1);
    expect_tx_revert(update, &update_socket, ExecutionError::EpochHasNotStarted).await;
}

#[tokio::test]
async fn test_epoch_change_reverts_not_participating() {
    let temp_dir = tempdir().unwrap();

    // Create a genesis committee and seed the application state with it.
    let committee_size = 4;
    let (committee, _keystore) = create_genesis_committee(committee_size);
    let (update_socket, query_runner) = test_init_app(&temp_dir, committee);

    let owner_secret_key = AccountOwnerSecretKey::generate();
    // New Node key
    let node_secret_key = NodeSecretKey::generate();

    // Stake less than the minimum required amount.
    let minimum_stake_amount: HpUfixed<18> = query_runner.get_staking_amount().into();

    deposit_and_stake(
        &update_socket,
        &owner_secret_key,
        1,
        &minimum_stake_amount,
        &node_secret_key.to_pk(),
        [0; 96].into(),
    )
    .await;

    // Execute opt-in transaction.
    // The node will only participate once the next epoch starts.
    expect_tx_success(
        prepare_update_request_node(UpdateMethod::OptIn {}, &node_secret_key, 1),
        &update_socket,
        ExecutionData::None,
    )
    .await;

    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 0 };
    let update = prepare_update_request_node(change_epoch, &node_secret_key, 2);
    expect_tx_revert(update, &update_socket, ExecutionError::NodeNotParticipating).await;
}

#[tokio::test]
async fn test_epoch_change_reverts_already_signaled() {
    let temp_dir = tempdir().unwrap();

    // Create a genesis committee and seed the application state with it.
    let committee_size = 4;
    let (committee, keystore) = create_genesis_committee(committee_size);
    let (update_socket, _query_runner) = test_init_app(&temp_dir, committee);

    let change_epoch = UpdateMethod::ChangeEpoch { epoch: 0 };
    let update = prepare_update_request_node(change_epoch.clone(), &keystore[0].node_secret_key, 1);
    expect_tx_success(update, &update_socket, ExecutionData::None).await;

    // Second update
    let update = prepare_update_request_node(change_epoch, &keystore[0].node_secret_key, 2);
    expect_tx_revert(update, &update_socket, ExecutionError::AlreadySignaled).await;
}

#[tokio::test]
async fn test_distribute_rewards() {
    let commit_phase_duration = 2000;
    let reveal_phase_duration = 2000;
    let mut network = TestNetwork::builder()
        .with_mock_consensus(MockConsensusConfig {
            block_buffering_interval: Duration::from_millis(100),
            max_ordering_time: 1,
            ..Default::default()
        })
        .with_committee_beacon_config(CommitteeBeaconConfig::default())
        .with_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(4)
        .await
        .with_genesis_mutator(move |genesis| {
            genesis.max_inflation = 10;
            genesis.node_share = 80;
            genesis.protocol_share = 10;
            genesis.service_builder_share = 10;
            genesis.max_boost = 4;
            genesis.supply_at_genesis = 1_000_000;
            genesis.committee_selection_beacon_commit_phase_duration = commit_phase_duration;
            genesis.committee_selection_beacon_reveal_phase_duration = reveal_phase_duration;
        })
        .build()
        .await
        .unwrap();
    let genesis = &network.genesis;
    let node1 = network.node(0);
    let node2 = network.node(1);

    // Initialize params for emission calculations.
    let percentage_divisor: HpUfixed<18> = 100_u16.into();
    let supply_at_year_start: HpUfixed<18> = genesis.supply_at_genesis.into();
    let inflation: HpUfixed<18> = HpUfixed::from(genesis.max_inflation) / &percentage_divisor;
    let node_share = HpUfixed::from(genesis.node_share) / &percentage_divisor;
    let protocol_share = HpUfixed::from(genesis.protocol_share) / &percentage_divisor;
    let service_share = HpUfixed::from(genesis.service_builder_share) / &percentage_divisor;

    // Deposit and stake FLK tokens, and stake lock in node 2.
    network
        .node(0)
        .downcast::<TestFullNodeComponentsWithMockConsensus>()
        .deposit_and_stake(10_000_u64.into(), &node1.get_owner_secret_key())
        .await
        .unwrap();
    node2
        .downcast::<TestFullNodeComponentsWithMockConsensus>()
        .deposit_and_stake(10_000_u64.into(), &node2.get_owner_secret_key())
        .await
        .unwrap();
    node2
        .downcast::<TestFullNodeComponentsWithMockConsensus>()
        .stake_lock(1460, &node2.get_owner_secret_key())
        .await
        .unwrap();

    // Build delivery acknowledgment transactions.
    let commodity_10 = 12_800;
    let commodity_11 = 3_600;
    let commodity_21 = 5000;
    let pod_10 = UpdateMethod::SubmitDeliveryAcknowledgmentAggregation {
        commodity: commodity_10,
        service_id: 0,
        proofs: vec![DeliveryAcknowledgmentProof],
        metadata: None,
    };
    let pod_11 = UpdateMethod::SubmitDeliveryAcknowledgmentAggregation {
        commodity: commodity_11,
        service_id: 1,
        proofs: vec![DeliveryAcknowledgmentProof],
        metadata: None,
    };
    let pod_21 = UpdateMethod::SubmitDeliveryAcknowledgmentAggregation {
        commodity: commodity_21,
        service_id: 1,
        proofs: vec![DeliveryAcknowledgmentProof],
        metadata: None,
    };

    let node_1_usd = 0.1 * (commodity_10 as f64) + 0.2 * (commodity_11 as f64); // 2_000 in revenue
    let node_2_usd = 0.2 * (commodity_21 as f64); // 1_000 in revenue
    let reward_pool: HpUfixed<6> = (node_1_usd + node_2_usd).into();

    let node_1_proportion: HpUfixed<18> = HpUfixed::from(2000_u64) / HpUfixed::from(3000_u64);
    let node_2_proportion: HpUfixed<18> = HpUfixed::from(1000_u64) / HpUfixed::from(3000_u64);

    let service_proportions: Vec<HpUfixed<18>> = vec![
        HpUfixed::from(1280_u64) / HpUfixed::from(3000_u64),
        HpUfixed::from(1720_u64) / HpUfixed::from(3000_u64),
    ];

    // Execute delivery acknowledgment transactions.
    node1.execute_transaction_from_node(pod_10).await.unwrap();
    node1.execute_transaction_from_node(pod_11).await.unwrap();
    node2.execute_transaction_from_node(pod_21).await.unwrap();

    // Trigger epoch change and distribute rewards.
    network
        .change_epoch_and_wait_for_complete(0, commit_phase_duration, reveal_phase_duration)
        .await
        .unwrap();

    // Check node stables balances.
    assert_eq!(
        node1
            .app_query()
            .get_account_info(&node1.get_owner_address(), |a| a.stables_balance)
            .unwrap(),
        HpUfixed::<6>::from(node_1_usd) * node_share.convert_precision()
    );
    assert_eq!(
        node1
            .app_query()
            .get_account_info(&node2.get_owner_address(), |a| a.stables_balance)
            .unwrap(),
        HpUfixed::<6>::from(node_2_usd) * node_share.convert_precision()
    );

    // Calculate emissions per unit.
    let emissions: HpUfixed<18> =
        (inflation * supply_at_year_start) / &genesis.epochs_per_year.into();
    let emissions_for_node = &emissions * &node_share;

    // Check node FLK balances.
    let total_share =
        &node_1_proportion * HpUfixed::from(1_u64) + &node_2_proportion * HpUfixed::from(4_u64);
    assert_eq!(
        node1
            .app_query()
            .get_account_info(&node1.get_owner_address(), |a| a.flk_balance)
            .unwrap(),
        (&emissions_for_node * &node_1_proportion) / &total_share
    );
    assert_eq!(
        node2
            .app_query()
            .get_account_info(&node2.get_owner_address(), |a| a.flk_balance)
            .unwrap(),
        (&emissions_for_node * (&node_2_proportion * HpUfixed::from(4_u64))) / &total_share
    );

    // Check the protocol fund balances.
    let protocol_account = node1.app_query().get_protocol_fund_address().unwrap();
    let protocol_balance = node1
        .app_query()
        .get_account_info(&protocol_account, |a| a.flk_balance)
        .unwrap();
    let protocol_rewards = &emissions * &protocol_share;
    assert_eq!(protocol_balance, protocol_rewards);

    let protocol_stables_balance = node1
        .app_query()
        .get_account_info(&protocol_account, |a| a.stables_balance)
        .unwrap();
    assert_eq!(
        &reward_pool * &protocol_share.convert_precision(),
        protocol_stables_balance
    );

    // Check the service owner balances.
    for s in 0..2 {
        let service_owner = node1.app_query().get_service_info(&s).unwrap().owner;
        let service_balance = node1
            .app_query()
            .get_account_info(&service_owner, |a| a.flk_balance)
            .unwrap();
        assert_eq!(
            service_balance,
            &emissions * &service_share * &service_proportions[s as usize]
        );
        let service_stables_balance = node1
            .app_query()
            .get_account_info(&service_owner, |a| a.stables_balance)
            .unwrap();
        assert_eq!(
            service_stables_balance,
            &reward_pool
                * &service_share.convert_precision()
                * &service_proportions[s as usize].convert_precision()
        );
    }

    // Shutdown the network.
    network.shutdown().await;
}

#[tokio::test]
async fn test_supply_across_epoch() {
    let commit_phase_duration = 2000;
    let reveal_phase_duration = 2000;
    let mut network = TestNetwork::builder()
        .with_mock_consensus(MockConsensusConfig {
            block_buffering_interval: Duration::from_millis(100),
            max_ordering_time: 1,
            ..Default::default()
        })
        .with_committee_beacon_config(CommitteeBeaconConfig::default())
        .with_committee_nodes::<TestFullNodeComponentsWithMockConsensus>(4)
        .await
        .with_genesis_mutator(move |genesis| {
            genesis.epoch_time = 100;
            genesis.epochs_per_year = 3;
            genesis.max_inflation = 10;
            genesis.node_share = 80;
            genesis.protocol_share = 10;
            genesis.service_builder_share = 10;
            genesis.max_boost = 4;
            genesis.supply_at_genesis = 1000000;
            genesis.committee_selection_beacon_commit_phase_duration = commit_phase_duration;
            genesis.committee_selection_beacon_reveal_phase_duration = reveal_phase_duration;
        })
        .build()
        .await
        .unwrap();
    let genesis = &network.genesis;
    let node = network.node(0);

    // Initialize params for emission calculations.
    let percentage_divisor: HpUfixed<18> = 100_u16.into();
    let supply_at_year_start: HpUfixed<18> = genesis.supply_at_genesis.into();
    let inflation: HpUfixed<18> = HpUfixed::from(genesis.max_inflation) / &percentage_divisor;
    let node_share = HpUfixed::from(genesis.node_share) / &percentage_divisor;
    let protocol_share = HpUfixed::from(genesis.protocol_share) / &percentage_divisor;
    let service_share = HpUfixed::from(genesis.service_builder_share) / &percentage_divisor;

    // Deposit and stake FLK tokens.
    node.downcast::<TestFullNodeComponentsWithMockConsensus>()
        .deposit_and_stake(10_000_u64.into(), &node.get_owner_secret_key())
        .await
        .unwrap();

    // Calculate emissions per unit.
    let emissions_per_epoch: HpUfixed<18> =
        (&inflation * &supply_at_year_start) / &genesis.epochs_per_year.into();

    // Get supply at this point.
    let mut supply = supply_at_year_start;

    // Iterate through `epoch_per_year` epoch changes to see if the current supply and year start
    // supply are as expected.
    for epoch in 0..genesis.epochs_per_year {
        // Add at least one transaction per epoch, so reward pool is not zero.
        node.execute_transaction_from_node(UpdateMethod::SubmitDeliveryAcknowledgmentAggregation {
            commodity: 10000,
            service_id: 0,
            proofs: vec![DeliveryAcknowledgmentProof],
            metadata: None,
        })
        .await
        .unwrap();

        // We have to submit uptime measurements to make sure nodes aren't set to
        // participating=false in the next epoch.
        for node in network.nodes() {
            let mut map = BTreeMap::new();
            let measurements = test_reputation_measurements(100);

            for peer in network.nodes() {
                if node.get_node_secret_key() == peer.get_node_secret_key() {
                    continue;
                }

                map.insert(peer.index(), measurements.clone());
            }
            node.execute_transaction_from_node(UpdateMethod::SubmitReputationMeasurements {
                measurements: map,
            })
            .await
            .unwrap();
        }

        // Trigger epoch change and distribute rewards.
        // Start commit phase.
        let new_epoch = network.change_epoch().await.unwrap();

        // Wait for the commit phase to end.
        tokio::time::sleep(Duration::from_millis(commit_phase_duration)).await;
        // Send the commit phase timeout transaction from 2/3+1 committee nodes.
        network.commit_phase_timeout(0).await.unwrap();

        // Wait for the reveal phase to end.
        tokio::time::sleep(Duration::from_millis(reveal_phase_duration)).await;
        // Send the reveal phase timeout transaction from 2/3+1 committee nodes.
        network.reveal_phase_timeout(0).await.unwrap();

        // Wait for the epoch change to complete.
        network.wait_for_epoch_change(new_epoch).await.unwrap();

        // Check that the total supply was updated correctly.
        let supply_increase = &emissions_per_epoch * &node_share
            + &emissions_per_epoch * &protocol_share
            + &emissions_per_epoch * &service_share;
        let total_supply = node.app_query().get_total_supply().unwrap();
        supply += supply_increase;
        assert_eq!(total_supply, supply);

        // If this is the last epoch, check if the supply_year_start is updated correctly.
        if epoch == genesis.epochs_per_year - 1 {
            let supply_year_start = node.app_query().get_supply_year_start().unwrap();
            assert_eq!(total_supply, supply_year_start);
        }
    }

    // Shutdown the network.
    network.shutdown().await;
}
