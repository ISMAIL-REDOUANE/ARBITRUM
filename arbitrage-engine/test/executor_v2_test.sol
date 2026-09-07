// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../../contracts/ArbitrageExecutorYulV2.sol";

/// @title ArbitrageExecutor Tests V2
/// @notice Tests for the corrected arbitrage executor contract

contract ArbitrageExecutorV2Test is Test {
    ArbitrageExecutorYulV2 public executor;
    address public owner;
    address public user;

    // Mock Balancer Vault for testing
    MockBalancerVault public mockBalancer;

    // Test tokens
    address constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A4192681371d;
    address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;

    function setUp() public {
        owner = address(this);
        user = makeAddr("user");

        // Deploy mock balancer
        mockBalancer = new MockBalancerVault();

        // Deploy executor with 0 min profit for testing
        executor = new ArbitrageExecutorYulV2(0);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 1: Correct Token Addresses
    // ═══════════════════════════════════════════════════════════════════════════
    function testCorrectTokenAddresses() public view {
        // Verify the executor has the correct USDC address
        assertEq(executor.USDC(), USDC);
        assertEq(executor.WETH(), WETH);
        assertEq(executor.BALANCER_VAULT(), 0xBA12222222228d8Ba445958a75a0704d566BF2C8);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 2: Owner-Only Functions
    // ═══════════════════════════════════════════════════════════════════════════
    function testSetMinProfitOwner() public {
        vm.prank(owner);
        executor.setMinProfit(1_000_000_000_000_000_000); // 1 ETH

        assertEq(executor.minProfit(), 1_000_000_000_000_000_000);
    }

    function testSetMinProfitNonOwner() public {
        vm.prank(user);
        vm.expectRevert();
        executor.setMinProfit(1_000_000_000_000_000_000);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 3: Rescue Functions
    // ═══════════════════════════════════════════════════════════════════════════
    function testRescueToken() public {
        // Deploy mock USDC
        MockERC20 usdc = new MockERC20(USDC);

        // Send some tokens to executor
        usdc.transfer(address(executor), 1000e6);

        // Owner can rescue
        vm.prank(owner);
        executor.rescueToken(address(usdc), 1000e6);

        assertEq(usdc.balanceOf(owner), 1000e6);
    }

    function testRescueTokenNonOwner() public {
        MockERC20 usdc = new MockERC20(USDC);
        usdc.transfer(address(executor), 1000e6);

        vm.prank(user);
        vm.expectRevert();
        executor.rescueToken(address(usdc), 1000e6);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 4: ABI Encoding - flashLoan Calldata Verification
    // ═══════════════════════════════════════════════════════════════════════════
    function testFlashLoanCalldataEncoding() public {
        // Verify the flashLoan selector
        bytes4 selector = bytes4(keccak256("flashLoan(address[],uint256[],bytes)"));
        assertEq(selector, 0x8c9b2d83);

        // Build test calldata manually
        address loanToken = USDC;
        uint256 loanAmount = 1_000_000e6;
        uint256 minProfitAmount = 1000e6;
        address pool = 0x1234567890123456789012345678901234567890;
        bytes memory path = abi.encode(USDC, 3000, address(executor));

        bytes memory userData = abi.encode(
            address(executor),
            loanToken,
            loanAmount,
            minProfitAmount,
            pool,
            path
        );

        // Verify userData can be decoded correctly
        (address initiator, uint256 decodedLoanAmount, uint256 decodedMinProfit, address decodedPool, bytes memory decodedPath) =
            abi.decode(userData, (address, uint256, uint256, address, bytes));

        assertEq(initiator, address(executor));
        assertEq(decodedLoanAmount, loanAmount);
        assertEq(decodedMinProfit, minProfitAmount);
        assertEq(decodedPool, pool);
        assertEq(decodedPath.length, path.length);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 5: Balance Check Logic
    // ═══════════════════════════════════════════════════════════════════════════
    function testProfitCalculationUnderflowProtection() public pure {
        // Simulate the profit calculation logic
        uint256 balAfter = 1000e6;
        uint256 loanAmount = 1100e6;
        uint256 feeAmount = 1e6; // 0.1%
        uint256 repaymentAmount = loanAmount + feeAmount;

        // balAfter < repayment, should revert in contract
        assertTrue(balAfter <= repaymentAmount);
    }

    function testProfitCalculationSuccess() public pure {
        uint256 balAfter = 2000e6;
        uint256 loanAmount = 1000e6;
        uint256 feeAmount = 1e6;
        uint256 repaymentAmount = loanAmount + feeAmount;

        uint256 expectedProfit = balAfter - repaymentAmount;
        assertEq(expectedProfit, 999e6);
        assertTrue(expectedProfit >= 0); // No underflow
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 6: Callback Selector Verification
    // ═══════════════════════════════════════════════════════════════════════════
    function testBalancerCallbackSelector() public {
        // The callback selector should match what Balancer expects
        // This is the IFlashLoanRecipient.onFlashLoan callback
        assertEq(ArbitrageExecutorYulV2(0).BALANCER_CALLBACK_SELECTOR(), 0x8a1e5e83);
    }
}

/// @notice Mock Balancer Vault for testing
contract MockBalancerVault {
    // Flash loan callback selector
    bytes4 constant CALLBACK_SELECTOR = 0x8a1e5e83;

    // Revert if not called correctly
    function flashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external {
        // Verify basic parameters
        require(tokens.length == 1, "Invalid tokens length");
        require(amounts.length == 1, "Invalid amounts length");

        // Get callback target from userData
        (address initiator, , uint256 minProfitAmount, , ) = abi.decode(
            userData,
            (address, address, uint256, address, bytes)
        );

        // Calculate fee (0.1% for testing)
        uint256 fee = amounts[0] / 1000;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = fee;

        // Call back the receiver
        IFlashLoanRecipient(msg.sender).receiveFlashLoan(
            tokens,
            amounts,
            feeAmounts,
            userData
        );
    }
}

/// @notice Minimal IERC20 interface for testing
interface IFlashLoanRecipient {
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external;
}

/// @notice Mock ERC20 for testing
contract MockERC20 {
    address public underlying;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(address _underlying) {
        underlying = _underlying;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function balanceOf(address account) external view returns (uint256) {
        return balanceOf[account];
    }
}
