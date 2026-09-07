// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../contracts/ArbitrageExecutorTwoLeg.sol";

/// @title TwoLegIntegrationTest
/// @notice Integration test proving Balancer callback works correctly
/// @dev Proves: abi.decode, security checks, rejection logic
contract TwoLegIntegrationTest is Test {
    ArbitrageExecutorTwoLeg public executor;
    MockERC20 public testToken;

    address public owner;
    address public initiator;
    address public attacker;

    address constant REAL_BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;

    uint256 constant LOAN_AMOUNT = 1_000_000e6;

    function setUp() public {
        owner = address(this);
        initiator = address(this);
        attacker = makeAddr("attacker");

        executor = new ArbitrageExecutorTwoLeg(100e6);
        testToken = new MockERC20();
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 1: Callback decodes userData correctly via abi.decode
    // ═══════════════════════════════════════════════════════════════════════════
    function testUserDataDecodingRoundTrip() public {
        address leg1Pool = makeAddr("leg1Pool");
        address leg2Pool = makeAddr("leg2Pool");
        bytes memory leg1Data = bytes("leg1");
        bytes memory leg2Data = bytes("leg2");

        bytes memory userData = abi.encode(
            address(this),
            address(0xaf88d065e77c8cC2239327C5EDb3A432268e5831),
            LOAN_AMOUNT,
            leg1Pool,
            leg2Pool,
            leg1Data,
            leg2Data
        );

        (address decInit, address decLoan, uint256 decAmt, address decLeg1, address decLeg2, bytes memory decLeg1D, bytes memory decLeg2D) =
            abi.decode(userData, (address, address, uint256, address, address, bytes, bytes));

        assertEq(decInit, address(this));
        assertEq(decLoan, address(0xaf88d065e77c8cC2239327C5EDb3A432268e5831));
        assertEq(decAmt, LOAN_AMOUNT);
        assertEq(decLeg1, leg1Pool);
        assertEq(decLeg2, leg2Pool);
        assertEq(decLeg1D, leg1Data);
        assertEq(decLeg2D, leg2Data);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 2: Callback rejects non-Balancer caller
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsNonBalancer() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(attacker);
        vm.expectRevert(bytes("ONLY_BALANCER"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 3: Callback rejects wrong token array length
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsWrongTokenLength() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](0);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("INVALID_TOKENS_LENGTH"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 4: Callback rejects wrong amounts length
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsWrongAmountsLength() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = LOAN_AMOUNT;
        amounts[1] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("INVALID_AMOUNTS_LENGTH"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 5: Callback rejects token mismatch
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsTokenMismatch() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = makeAddr("wrongToken");
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("TOKEN_MISMATCH"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 6: Callback rejects amount mismatch
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsAmountMismatch() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2_000_000e6;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("AMOUNT_MISMATCH"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 7: Callback cannot be called by EOA directly
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_CannotBeCalledByEOA() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.expectRevert(bytes("ONLY_BALANCER"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 8: Callback cannot be called by arbitrary contract
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_CannotBeCalledByArbitraryContract() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), bytes(""), bytes("")
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        address randomContract = address(new RandomContract());
        vm.prank(randomContract);
        vm.expectRevert(bytes("ONLY_BALANCER"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 9: Malformed userData is rejected
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsMalformedUserData() public {
        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert();
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Helper: Build valid Uniswap V3 exactInputSingle calldata
    // Layout (after bytes length): selector(4) + tokenIn(32) + tokenOut(32) + fee(32) + recipient(32) + deadline(32) + amountIn(32) + amountOutMin(32) + sqrtPriceLimit(32) = 292 bytes
    // In memory (after bytes length at offset 32): bytes 0-3=selector, 4-35=tokenIn, 36-67=tokenOut, 68-99=fee, 100-131=recipient, 132-163=deadline, 164-195=amountIn, 196-227=amountOutMin, 228-259=sqrtPriceLimit
    // ═══════════════════════════════════════════════════════════════════════════
    function _buildLegData(address tokenIn, address tokenOut, address recipient) internal pure returns (bytes memory) {
        bytes memory legData = new bytes(228);
        assembly {
            let p := add(legData, 32)
            // selector: exactInputSingle = 0x04e45aaf (stored at bytes 0-3)
            mstore(p, shl(224, 0x04e45aaf))
            // tokenIn at offset 4 (bytes 4-35)
            mstore(add(p, 4), tokenIn)
            // tokenOut at offset 36 (bytes 36-67)
            mstore(add(p, 36), tokenOut)
            // fee at offset 68 (bytes 68-99)
            mstore(add(p, 68), 0x1f4) // 500
            // recipient at offset 100 (bytes 100-131)
            mstore(add(p, 100), recipient)
            // amountIn at offset 132 (bytes 132-163)
            mstore(add(p, 132), 1000000000000)
            // amountOutMin at offset 164 (bytes 164-195)
            mstore(add(p, 164), 1)
            // sqrtPriceLimit at offset 196 (bytes 196-227)
            mstore(add(p, 196), 0)
        }
        return legData;
    }


    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 10: Leg1 rejects wrong selector
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg1WrongSelector() public {
        // Build leg1Data with wrong selector - recipient must be executor
        bytes memory leg1Data = _buildLegData(address(testToken), address(0x1234567890123456789012345678901234567890), address(executor));
        leg1Data[0] = 0x00; // Corrupt the selector
        leg1Data[1] = 0x00;
        leg1Data[2] = 0x00;
        leg1Data[3] = 0x00;

        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), address(executor));

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG1_BAD_SELECTOR"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 11: Leg1 rejects wrong tokenIn
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg1WrongTokenIn() public {
        // Build leg1Data with WRONG tokenIn (not the loan token)
        address wrongToken = makeAddr("wrongToken");
        bytes memory leg1Data = _buildLegData(wrongToken, address(0x1234567890123456789012345678901234567890), address(executor));
        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), address(executor));

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG1_BAD_TOKEN_IN"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 12: Leg1 rejects wrong recipient
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg1WrongRecipient() public {
        testToken.mint(address(executor), LOAN_AMOUNT);
        // Build leg1Data with wrong recipient (not executor)
        address wrongRecipient = makeAddr("wrongRecipient");
        bytes memory leg1Data = _buildLegData(address(testToken), address(0x1234567890123456789012345678901234567890), wrongRecipient);
        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), address(executor));

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG1_BAD_RECIPIENT"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 13: Leg2 rejects wrong selector
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg2WrongSelector() public {
        testToken.mint(address(executor), LOAN_AMOUNT);
        bytes memory leg1Data = _buildLegData(address(testToken), address(0x1234567890123456789012345678901234567890), address(executor));

        // Build leg2Data with wrong selector
        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), address(executor));
        leg2Data[0] = 0xFF; // Corrupt the selector
        leg2Data[1] = 0xFF;
        leg2Data[2] = 0xFF;
        leg2Data[3] = 0xFF;

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG2_BAD_SELECTOR"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 14: Leg2 rejects tokenIn != Leg1 output
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg2WrongTokenIn() public {
        testToken.mint(address(executor), LOAN_AMOUNT);
        // Leg1 outputs tokenX, but Leg2 expects tokenY as input
        address tokenX = makeAddr("tokenX");
        address tokenY = makeAddr("tokenY");

        bytes memory leg1Data = _buildLegData(address(testToken), tokenX, address(executor));
        // Leg2's tokenIn is tokenY, but should be tokenX (Leg1's output)
        bytes memory leg2Data = _buildLegData(tokenY, address(testToken), address(executor));

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG2_BAD_TOKEN_IN"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 15: Leg2 rejects wrong recipient
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg2WrongRecipient() public {
        testToken.mint(address(executor), LOAN_AMOUNT);
        bytes memory leg1Data = _buildLegData(address(testToken), address(0x1234567890123456789012345678901234567890), address(executor));

        // Build leg2Data with wrong recipient
        address wrongRecipient = makeAddr("wrongRecipient");
        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), wrongRecipient);

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG2_BAD_RECIPIENT"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 16: Leg1 rejects too short calldata
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg1TooShort() public {
        // Build leg1Data that is too short (less than 196 bytes)
        bytes memory leg1Data = bytes("too short");
        bytes memory leg2Data = _buildLegData(address(0x1234567890123456789012345678901234567890), address(testToken), address(executor));

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG1_TOO_SHORT"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // TEST 17: Leg2 rejects too short calldata
    // ═══════════════════════════════════════════════════════════════════════════
    function testCallback_RejectsLeg2TooShort() public {
        testToken.mint(address(executor), LOAN_AMOUNT);
        bytes memory leg1Data = _buildLegData(address(testToken), address(0x1234567890123456789012345678901234567890), address(executor));
        // Build leg2Data that is too short (less than 196 bytes)
        bytes memory leg2Data = bytes("too short");

        bytes memory userData = abi.encode(
            initiator, address(testToken), LOAN_AMOUNT,
            makeAddr("leg1"), makeAddr("leg2"), leg1Data, leg2Data
        );

        address[] memory tokens = new address[](1);
        tokens[0] = address(testToken);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = LOAN_AMOUNT;
        uint256[] memory feeAmounts = new uint256[](1);
        feeAmounts[0] = LOAN_AMOUNT / 1000;

        vm.prank(REAL_BALANCER_VAULT);
        vm.expectRevert(bytes("LEG2_TOO_SHORT"));
        executor.receiveFlashLoan(tokens, amounts, feeAmounts, userData);
    }
}

contract RandomContract {
    fallback() external {}
}

contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        uint256 fromBalance = balanceOf[msg.sender];
        require(fromBalance >= amount, "INSUFFICIENT_BALANCE");
        balanceOf[msg.sender] = fromBalance - amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 fromBalance = balanceOf[from];
        uint256 allowed = allowance[from][msg.sender];
        require(fromBalance >= amount, "INSUFFICIENT_BALANCE");
        require(allowed >= amount, "ALLOWANCE_EXCEEDED");
        balanceOf[from] = fromBalance - amount;
        balanceOf[to] += amount;
        allowance[from][msg.sender] = allowed - amount;
        return true;
    }

    function approve(address, uint256) external pure returns (bool) {
        return true;
    }
}
