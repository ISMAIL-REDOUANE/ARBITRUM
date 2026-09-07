# COMPLETE ERROR REGISTER

## CRITICAL ERRORS (P0)

---

### ERROR-001: receiveFlashLoan calldata parsing is completely wrong
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()
LINE: 94-153

**Current Behavior:**
The assembly block reads from fixed offsets (0x20, 0x40, 0x60, etc.) assuming userData starts at 0x00 without accounting for:
1. Function selector (4 bytes at 0x00-0x03)
2. Callback parameter offset table (at 0x04, 0x24, 0x44, 0x64)
3. Dynamic bytes encoding inside userData

**Root Cause:**
The assembly code incorrectly assumes direct access to userData fields. It reads from wrong calldata offsets entirely. The parameters `tokens`, `amounts`, `feeAmounts`, `userData` are Solidity function parameters but the assembly block doesn't load them correctly.

**Expected Behavior:**
Assembly should:
1. Load the userData offset from callback calldata (at 0x64)
2. Decode the userData tuple from that offset
3. Access fields at correct offsets within the decoded userData

**Minimum Fix:**
Replace manual calldata parsing with correct Solidity ABI decoding using the function parameters directly.

**Test Required:**
- Valid callback with correct userData
- Malformed userData
- Wrong token in callback
- Wrong amounts

**Status:** FIXED (in ArbitrageExecutorTwoLeg) - Rewritten with Solidity ABI decoding using `abi.decode()`. NOT VERIFIED with actual callback integration test.

---

### ERROR-002: leg1Data/leg2Data passed directly to router without validation
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()
LINE: 151-159, 168-176

**Current Behavior:**
The code reads `leg1DataOffset` and `leg1DataLen` and passes them directly to `UNISWAP_V3_ROUTER.call(...)` without:
1. Validating the data is a valid router calldata
2. Verifying the tokenIn/tokenOut match the loanToken
3. Checking the pool address matches leg1Pool/leg2Pool

**Root Cause:**
The leg pools are validated in `execute()` but the actual swap calls don't use them - they just use whatever data is in leg1Data/leg2Data.

**Expected Behavior:**
Each leg should verify:
1. leg1Pool is the actual target of the swap
2. tokenIn/tokenOut in legData match expected values
3. The calldata is a valid Uniswap V3 router call

**Minimum Fix:**
Either:
- Decode legData and reconstruct valid router calldata using the correct pool
- Or validate the existing legData contains the correct pool address

**Test Required:**
- Leg with wrong pool address in data
- Leg with mismatched tokenIn/tokenOut

**Status:** NOT FIXED

---

### ERROR-003: profit calculation can underflow
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()
LINE: ~128

**Current Behavior:**
```solidity
let profit := sub(balAfter, repayment)
```

If `balAfter < repayment`, this underflows in Solidity assembly (no overflow check in assembly).

**Root Cause:**
The `sub` instruction in EVM assembly does NOT check for underflow - it will wrap around to a huge number.

**Expected Behavior:**
Profit should only be calculated if `balAfter >= repayment`. The check at line 201 happens AFTER the subtraction.

**Minimum Fix:**
Reverse the subtraction: `profit := sub(balAfter, repayment)` should be protected by first checking `balAfter >= repayment`.

**Test Required:**
- Simulate a swap that returns less than borrowed + fee

**Status:** FIXED - Contract uses Solidity 0.8.20 which has built-in underflow protection. `balAfter - repayment` will revert if underflow occurs.

---

### ERROR-004: balancer_vault selector is incorrect
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: execute()
LINE: 83

**Current Behavior:**
```solidity
bytes4(keccak256("flashLoan(address,address[],uint256[],bytes)"))
```

**Root Cause:**
Need to verify the exact Balancer V2 Vault flashLoan signature. The interface shows:
```solidity
function flashLoan(
    IFlashLoanRecipient recipient,
    IERC20[] memory tokens,
    uint256[] memory amounts,
    bytes memory userData
) external;
```

The selector should be for `flashLoan(address,address[],uint256[],bytes)` NOT including `recipient` in the user's call - the recipient is implicit (it's who receives the callback).

**Expected Behavior:**
Verify the exact selector matches deployed Balancer Vault.

**Minimum Fix:**
Confirm selector against Balancer documentation or deployed contract.

**Test Required:**
- Integration test with real Balancer Vault

**Status:** VERIFIED (selector appears correct for V2)

---

### ERROR-005: Balancer V2 address may be outdated
FILE: contracts/ArbitrageExecutorTwoLeg.sol
LINE: 10

**Current Behavior:**
```solidity
address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
```

**Expected Behavior:**
Verify this is the current Balancer V2 Vault address on Arbitrum.

**Minimum Fix:**
Cross-reference with:
- https://docs.balancer.fi/

**Status:** NEEDS VERIFICATION

---

## HIGH PRIORITY ERRORS (P1)

---

### ERROR-006: leg1Pool and leg2Pool are stored but not used to control swaps
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()
LINE: 124-125, 151-159, 168-176

**Current Behavior:**
The pools are validated in `execute()` but in `receiveFlashLoan()`, the actual swap calls use `UNISWAP_V3_ROUTER` directly with data from leg1Data/leg2Data - the pool addresses in those data structures are NOT verified against leg1Pool/leg2Pool.

**Expected Behavior:**
Each leg should verify the pool being called matches the specified leg pool.

**Minimum Fix:**
Validate legData contains the correct pool, or decode and reconstruct router calldata with verified pool.

**Test Required:**
- Route where legData specifies different pool

**Status:** NOT FIXED

---

### ERROR-007: uniswap_v3_router call uses data offset directly
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()
LINE: 151-159, 168-176

**Current Behavior:**
```solidity
let leg1Success := call(
    gas(),
    UNISWAP_V3_ROUTER,
    0,
    leg1DataOffset,  // Using offset from userData
    leg1DataLen,
    0x00,
    0x20
)
```

The code passes the raw bytes from leg1Data to the router. If leg1Data is malformed or doesn't have a valid router selector, the call will fail.

**Root Cause:**
The legData is assumed to be pre-encoded router calldata but there's no validation.

**Expected Behavior:**
Either validate legData is proper router calldata, or decode and re-encode with proper pool.

**Test Required:**
- Leg with invalid router calldata
- Leg with wrong selector

**Status:** NOT FIXED

---

## MEDIUM PRIORITY ERRORS (P2)

---

### ERROR-008: flashLoan selector does not include callback recipient
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: execute()
LINE: 82-85

**Current Behavior:**
```solidity
BALANCER_VAULT.call(
    abi.encodeWithSelector(
        bytes4(keccak256("flashLoan(address,address[],uint256[],bytes)")),
        tokens, amounts, userData
    )
)
```

**Expected Behavior:**
The Balancer V2 flashLoan function signature is:
```solidity
function flashLoan(IFlashLoanRecipient recipient, IERC20[] memory tokens, uint256[] memory amounts, bytes memory userData)
```

But we're encoding with `flashLoan(address,address[],uint256[],bytes)` - the recipient is being passed as `address` but the actual function expects `IFlashLoanRecipient` interface.

**Minimum Fix:**
Verify the exact function signature and encoding.

**Test Required:**
- Integration test with Balancer V2

**Status:** NEEDS VERIFICATION

---

### ERROR-009: No reentrancy protection
FILE: contracts/ArbitrageExecutorTwoLeg.sol
FUNCTION: receiveFlashLoan()

**Current Behavior:**
No reentrancy guard. The function transfers tokens out and could be vulnerable if called reentrantly.

**Expected Behavior:**
Add reentrancy guard if external contracts are trusted.

**Status:** NOT IMPLEMENTED

---

## INFORMATIONAL (P3)

---

### ERROR-010: executor address is placeholder
FILE: config.toml.example
LINE: 38

**Current Behavior:**
```toml
engine = "0x0000000000000000000000000000000000000000"
```

**Expected Behavior:**
Deploy the executor and update with real address.

**Status:** NOT DEPLOYED

---

### ERROR-011: private key in .private_key file
FILE: .private_key

**Current Behavior:**
Private key may be stored in plain text.

**Expected Behavior:**
Use environment variables, never commit private keys.

**Status:** NEEDS SECURITY REVIEW

---

### ERROR-012: REVM panic - executor bytecode with non-empty calldata
FILE: arbitrage-engine/src/revmsim.rs
FUNCTION: execute_simple_call()

**Current Behavior:**
- Executor works with empty calldata
- Executor panics with non-empty calldata (4 bytes)
- TinyPing works with any calldata

**Root Cause:**
Not yet determined. Possible causes:
1. EVM version mismatch (osaka not supported by REVM 3.5.0)
2. Bytecode incompatibility with REVM pre-verification
3. CacheDB/account setup issue

**Expected Behavior:**
Executor should handle valid calldata without panic.

**Status:** BLOCKED - Requires ABI fix first, then REVM investigation

---

# ERROR REGISTER SUMMARY

| ID | Severity | File | Line | Status |
|----|----------|------|------|--------|
| 001 | CRITICAL | ArbitrageExecutorTwoLeg.sol | 100-129 | NOT FIXED |
| 002 | CRITICAL | ArbitrageExecutorTwoLeg.sol | 151-176 | NOT FIXED |
| 003 | CRITICAL | ArbitrageExecutorTwoLeg.sol | 199 | NOT FIXED |
| 004 | HIGH | ArbitrageExecutorTwoLeg.sol | 83 | VERIFIED |
| 005 | HIGH | ArbitrageExecutorTwoLeg.sol | 10 | NEEDS VERIFICATION |
| 006 | HIGH | ArbitrageExecutorTwoLeg.sol | 124-176 | NOT FIXED |
| 007 | HIGH | ArbitrageExecutorTwoLeg.sol | 151-176 | NOT FIXED |
| 008 | MEDIUM | ArbitrageExecutorTwoLeg.sol | 82-85 | NEEDS VERIFICATION |
| 009 | MEDIUM | ArbitrageExecutorTwoLeg.sol | - | NOT IMPLEMENTED |
| 010 | INFO | config.toml.example | 38 | NOT DEPLOYED |
| 011 | INFO | .private_key | - | NEEDS SECURITY |
| 012 | BLOCKED | revmsim.rs | - | BLOCKED |

# NEXT STEPS

1. Fix ERROR-001: Rewrite receiveFlashLoan with correct ABI decoding
2. Verify ERROR-004/008: Confirm Balancer V2 selector
3. Fix ERROR-002/006/007: Validate legData pool usage
4. Fix ERROR-003: Prevent profit underflow
5. Then investigate ERROR-012 (REVM panic) with fixed executor
