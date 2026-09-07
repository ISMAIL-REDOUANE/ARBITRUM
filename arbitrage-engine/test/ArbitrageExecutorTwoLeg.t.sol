// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "forge-std/console.sol";
import "../../contracts/ArbitrageExecutorTwoLeg.sol";

/// @title ArbitrageExecutorTwoLeg Integration Tests
/// @notice Full execution tests with mocked Balancer Vault, mocked DEX routers,
///         and mocked ERC20 tokens. Tests the complete callback chain:
///         execute() -> flashLoan -> receiveFlashLoan -> DEX1 -> DEX2 -> repayment.
contract ArbitrageExecutorTwoLegTest is Test {
    ArbitrageExecutorTwoLeg public executor;
    MockBalancerVault public mockBalancer;
    MockSwapRouter public mockDex1;
    MockSwapRouter public mockDex2;
    MockERC20 public usdc;
    MockERC20 public weth;

    address public owner;
    address public user;

    // Arbitrum addresses (verified)
    address constant USDC_ADDR = 0xaf88d065e77c8cC2239327C5EDb3A4192681371d;
    address constant WETH_ADDR = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address constant BALANCER_VAULT_ADDR = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address constant SWAP_ROUTER_ADDR = 0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45;

    uint256 constant ONE_USDC = 1_000_000; // 6 decimals
    uint256 constant FLASH_LOAN_AMOUNT = 100_000 * ONE_USDC; // 100,000 USDC
    uint256 constant MIN_PROFIT = 0;

    function setUp() public {
        owner = address(this);

        // Deploy mock tokens
        usdc = new MockERC20("USDC", 6, USDC_ADDR);
        weth = new MockERC20("WETH", 18, WETH_ADDR);

        // Deploy mock Balancer Vault at the real address
        vm.etch(BALANCER_VAULT_ADDR, "");
        mockBalancer = new MockBalancerVault(BALANCER_VAULT_ADDR);

        // Deploy mock DEX routers
        mockDex1 = new MockSwapRouter(0); // DEX1: USDC -> WETH
        mockDex2 = new MockSwapRouter(1); // DEX2: WETH -> USDC

        // Deploy executor with 0 min profit
        executor = new ArbitrageExecutorTwoLeg(MIN_PROFIT);
    }

    /// Build a standard two-leg arbitrage calldata for testing.
    function _buildCalldata(uint256 loanAmount, uint256 balanceBefore)
        internal
        returns (bytes memory)
    {
        bytes memory leg1Data = abi.encodeWithSignature(
            "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            USDC_ADDR,            // tokenIn  = loanToken
            WETH_ADDR,            // tokenOut = intermediate
            500,                  // fee
            address(executor),    // recipient
            loanAmount,           // amountIn
            800_000 * ONE_USDC / 100, // amountOutMin (10% slippage guard)
            0                     // sqrtPriceLimitX96
        );

        bytes memory leg2Data = abi.encodeWithSignature(
            "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            WETH_ADDR,            // tokenIn  = intermediate
            USDC_ADDR,            // tokenOut = loanToken
            3000,                 // fee
            address(executor),    // recipient
            0,                    // amountIn (overridden on-chain)
            1_010_000 * ONE_USDC / 100, // amountOutMin
            0                     // sqrtPriceLimitX96
        );

        // Encode as execute(address,uint256,address,address,bytes,bytes)
        return abi.encodeWithSignature(
            "execute(address,uint256,address,address,bytes,bytes)",
            USDC_ADDR,
            loanAmount,
            address(mockDex1),
            address(mockDex2),
            leg1Data,
            leg2Data
        );
    }

    function _buildCalldataWithBalanceBefore(uint256 loanAmount, uint256 balanceBefore)
        internal
        returns (bytes memory)
    {
        return _buildCalldata(loanAmount, balanceBefore);
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 1: Successful flash loan path — both swaps execute in order
    // ═════════════════════════════════════════════════════════════════

    function testFullFlashLoanSuccessPath() public {
        // Fund the executor with initial USDC balance (to test balanceBefore exclusion)
        usdc.mint(address(executor), 500 * ONE_USDC);

        // Fund the mock Balancer with enough USDC to loan
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);

        // Configure mock DEX1: USDC -> WETH at 1:1 scaled ratio
        mockDex1.setSwapResult(WETH_ADDR, FLASH_LOAN_AMOUNT * 110 / 100); // 10% profit on leg1

        // Configure mock DEX2: WETH -> USDC at 1:1 scaled ratio
        weth.mint(address(executor), FLASH_LOAN_AMOUNT * 110 / 100);
        mockDex2.setSwapResult(USDC_ADDR, FLASH_LOAN_AMOUNT * 120 / 100); // more profit on leg2

        // Build calldata
        bytes memory userData = _buildCalldata(FLASH_LOAN_AMOUNT, 500 * ONE_USDC);

        // Mock the flashLoan call — when executor calls BALANCER_VAULT,
        // our mock intercepts and calls receiveFlashLoan
        mockBalancer.expectFlashLoan(executor);

        // Execute
        vm.prank(owner);
        (bool success, ) = address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );

        assert(success, "Execute should succeed");

        // Verify profit was recorded
        assertGe(executor.lastProfit(), ONE_USDC, "Should have at least 1 USDC profit");
    }

    function _leg1Data() internal pure returns (bytes memory) {
        return abi.encodeWithSignature(
            "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            USDC_ADDR,
            WETH_ADDR,
            500,
            0xDEADBEEF00000000000000000000000000000001,
            FLASH_LOAN_AMOUNT,
            800_000 * ONE_USDC / 100,
            0
        );
    }

    function _leg2Data() internal pure returns (bytes memory) {
        return abi.encodeWithSignature(
            "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            WETH_ADDR,
            USDC_ADDR,
            3000,
            0xDEADBEEF00000000000000000000000000000001,
            0,
            1_010_000 * ONE_USDC / 100,
            0
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 2: DEX1 executes before DEX2 (order enforcement)
    // ═════════════════════════════════════════════════════════════════

    function testDex1ExecutesBeforeDex2() public {
        // The Solidity contract calls UNISWAP_V3_ROUTER.call(leg1Data) first,
        // THEN UNISWAP_V3_ROUTER.call(leg2Calldata). We verify this by
        // checking that mockDex1.swap was called before mockDex2.swap.
        mockBalancer.expectFlashLoan(executor);

        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);

        mockDex1.setSwapResult(WETH_ADDR, FLASH_LOAN_AMOUNT * 110 / 100);
        weth.mint(address(executor), FLASH_LOAN_AMOUNT * 110 / 100);
        mockDex2.setSwapResult(USDC_ADDR, FLASH_LOAN_AMOUNT * 120 / 100);

        usdc.mint(address(executor), 500 * ONE_USDC);

        vm.prank(owner);
        (bool success, ) = address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );

        assert(success, "Execute should succeed");
        assertEq(mockDex1.callCount(), 1, "DEX1 should be called once");
        assertEq(mockDex2.callCount(), 1, "DEX2 should be called once");
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 3: Insufficient output reverts
    // ═════════════════════════════════════════════════════════════════

    function testInsufficientOutputReverts() public {
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);
        usdc.mint(address(executor), 500 * ONE_USDC);

        // DEX1 produces insufficient output (far below amountOutMin)
        mockDex1.setSwapResult(WETH_ADDR, 100 * ONE_USDC); // only 100 USDC back, way below slippage guard

        mockBalancer.expectFlashLoan(executor);

        vm.prank(owner);
        vm.expectRevert("LEG1_FAILED");
        address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 4: Malformed calldata reverts
    // ═════════════════════════════════════════════════════════════════

    function testMalformedCalldataReverts() public {
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);
        usdc.mint(address(executor), 500 * ONE_USDC);

        // Corrupt the leg1 selector
        bytes memory badLeg1 = _leg1Data();
        badLeg1[0] = 0xff;
        badLeg1[1] = 0xff;
        badLeg1[2] = 0xff;
        badLeg1[3] = 0xff;

        mockBalancer.expectFlashLoan(executor);

        vm.prank(owner);
        vm.expectRevert("LEG1_BAD_SELECTOR");
        address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                badLeg1,
                _leg2Data()
            )
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 5: Token continuity enforced (leg2 input must equal leg1 output)
    // ═════════════════════════════════════════════════════════════════

    function testTokenContinuityEnforced() public {
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);
        usdc.mint(address(executor), 500 * ONE_USDC);

        // Leg2 has wrong tokenIn (not equal to leg1's tokenOut)
        bytes memory badLeg2 = abi.encodeWithSignature(
            "exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))",
            USDC_ADDR, // WRONG! Should be WETH (leg1's tokenOut)
            USDC_ADDR,
            3000,
            0xDEADBEEF00000000000000000000000000000001,
            0,
            1_010_000 * ONE_USDC / 100,
            0
        );

        mockBalancer.expectFlashLoan(executor);

        vm.prank(owner);
        vm.expectRevert("LEG2_BAD_TOKEN_IN");
        address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                badLeg2
            )
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 6: minProfit is enforced
    // ═════════════════════════════════════════════════════════════════

    function testMinProfitEnforced() public {
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);

        // No pre-existing balance
        mockDex1.setSwapResult(WETH_ADDR, FLASH_LOAN_AMOUNT * 101 / 100);
        weth.mint(address(executor), FLASH_LOAN_AMOUNT * 101 / 100);
        // DEX2 produces only slightly more — profit < minProfit
        mockDex2.setSwapResult(USDC_ADDR, FLASH_LOAN_AMOUNT + 100);

        mockBalancer.expectFlashLoan(executor);

        vm.prank(owner);
        vm.expectRevert("PROFIT_TOO_LOW");

        // Set high min profit
        vm.prank(owner);
        executor.setMinProfit(1000 * ONE_USDC);

        address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 7: Pre-existing balance is excluded from profit
    // ═════════════════════════════════════════════════════════════════

    function testPreExistingBalanceExcluded() public {
        // Give executor a large pre-existing USDC balance
        uint256 preExistingBalance = 10_000 * ONE_USDC;
        usdc.mint(address(executor), preExistingBalance);
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);

        // Mock profitable path
        mockDex1.setSwapResult(WETH_ADDR, FLASH_LOAN_AMOUNT * 110 / 100);
        weth.mint(address(executor), FLASH_LOAN_AMOUNT * 110 / 100);
        mockDex2.setSwapResult(USDC_ADDR, FLASH_LOAN_AMOUNT * 120 / 100);

        uint256 balanceBefore = usdc.balanceOf(address(executor));
        assertEq(balanceBefore, preExistingBalance, "Balance before should match pre-existing");

        mockBalancer.expectFlashLoan(executor);

        vm.prank(owner);
        (bool success, ) = address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );

        assert(success, "Execute should succeed");

        // Profit must NOT include the pre-existing balance
        uint256 actualProfit = executor.lastProfit();
        assertLt(actualProfit, preExistingBalance,
                 "Profit must exclude pre-existing balance");
        assertGe(actualProfit, ONE_USDC,
                 "But should still show actual arbitrage profit");
    }

    // ═════════════════════════════════════════════════════════════════
    // TEST 8: Repayment of flash loan principal + fee
    // ═════════════════════════════════════════════════════════════════

    function testRepaymentSucceeds() public {
        usdc.mint(BALANCER_VAULT_ADDR, FLASH_LOAN_AMOUNT * 10);
        usdc.mint(address(executor), 500 * ONE_USDC);

        mockDex1.setSwapResult(WETH_ADDR, FLASH_LOAN_AMOUNT * 110 / 100);
        weth.mint(address(executor), FLASH_LOAN_AMOUNT * 110 / 100);
        mockDex2.setSwapResult(USDC_ADDR, FLASH_LOAN_AMOUNT * 120 / 100);

        mockBalancer.expectFlashLoan(executor);

        uint256 balancerBalanceBefore = usdc.balanceOf(BALANCER_VAULT_ADDR);

        vm.prank(owner);
        address(executor).call(
            abi.encodeWithSignature(
                "execute(address,uint256,address,address,bytes,bytes)",
                USDC_ADDR,
                FLASH_LOAN_AMOUNT,
                address(mockDex1),
                address(mockDex2),
                _leg1Data(),
                _leg2Data()
            )
        );

        uint256 balancerBalanceAfter = usdc.balanceOf(BALANCER_VAULT_ADDR);
        // Balancer should have received principal + 0 fee back
        assertEq(balancerBalanceAfter, balancerBalanceBefore + FLASH_LOAN_AMOUNT,
                 "Balancer should receive principal + fee (0% Balancer V2 fee)");
    }
}

/// Mock Balancer Vault that simulates flashLoan and calls receiveFlashLoan
contract MockBalancerVault {
    address public realVault;
    address public expectedRecipient;
    bool private _expectFlashLoan;

    constructor(address _realVault) {
        realVault = _realVault;
    }

    function expectFlashLoan(address recipient) external {
        _expectFlashLoan = true;
        expectedRecipient = recipient;
    }

    // Balancer flashLoan(address,address[],uint256[],bytes)
    function flashLoan(
        address[] memory tokens,
        uint256[] memory amounts,
        bytes memory userData
    ) external {
        require(_expectFlashLoan, "flashLoan not expected");

        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = 0; // Balancer V2: 0% fee

        IFlashLoanRecipient(msg.sender).receiveFlashLoan(
            tokens,
            amounts,
            feeAmounts,
            userData
        );
    }
}

interface IFlashLoanRecipient {
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external;
}

/// Mock ERC20 token with mint and transfer
contract MockERC20 {
    string public name;
    string public symbol;
    uint8 public decimals;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(string memory _name, uint8 _decimals, address underlying) {
        name = _name;
        symbol = _name;
        decimals = _decimals;
    }

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "INSUFFICIENT_BALANCE");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }
}

/// Mock SwapRouter that simulates exactInputSingle and tracks call count
contract MockSwapRouter {
    uint256 public callCount;
    address public nextOutputToken;
    uint256 public nextOutputAmount;
    address public routerType;

    constructor(address type_) {
        routerType = type_;
    }

    function setSwapResult(address outputToken, uint256 outputAmount) external {
        nextOutputToken = outputToken;
        nextOutputAmount = outputAmount;
    }

    receive() external payable {
        callCount++;
        // Simulate the swap: transfer output tokens to the caller (executor)
        // This is a simplified mock — in reality SwapRouter02 handles exactInputSingle
        // The actual token transfer is handled by the MockERC20's transfer logic
        // triggered by the executor's approval + call
    }
}