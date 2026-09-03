// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

contract SimpleFallback {
    fallback() external {
        assembly {
            mstore(0, 1)
            return(0, 32)
        }
    }
}
