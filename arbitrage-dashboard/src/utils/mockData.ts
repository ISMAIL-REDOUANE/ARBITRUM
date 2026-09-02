import type { 
  DashboardState, 
  EngineMetrics, 
  OpportunityFunnel, 
  RelayStats, 
  LatencySample, 
  TradeExecution,
  EngineStatus,
  RelayProvider,
  TradeStatus
} from '../types';

const ROUTES = [
  'UniswapV3 WETH/USDC -> Balancer Flash',
  'Balancer USDC -> UniswapV3 WETH',
  'SushiSwap WETH/DAI -> UniswapV3',
  'Curve stETH/ETH -> Balancer Flash',
  'UniswapV2 USDC/ETH -> UniswapV3',
  'Balancer WBTC/ETH -> UniswapV3 Flash',
  'UniswapV3 ETH/USDT -> SushiSwap',
  'Curve triCrypto -> Balancer USDC',
];

const TOKENS = ['USDC', 'ETH', 'USDT', 'WBTC', 'DAI'];
const RELAYS: RelayProvider[] = ['BloxRoute', 'Flashbots', 'MEV-Blocker', 'Public'];
const STATUSES: TradeStatus[] = ['Executed', 'Executed', 'Executed', 'Reverted', 'Dropped'];

function randomBetween(min: number, max: number): number {
  return Math.random() * (max - min) + min;
}

function randomIntBetween(min: number, max: number): number {
  return Math.floor(randomBetween(min, max + 1));
}

function generateTxHash(): string {
  const chars = '0123456789abcdef';
  let hash = '0x';
  for (let i = 0; i < 64; i++) {
    hash += chars[Math.floor(Math.random() * chars.length)];
  }
  return hash;
}

export function generateMockEngine(): EngineMetrics {
  const statuses: EngineStatus[] = ['Active', 'Active', 'Active', 'Idle'];
  return {
    status: statuses[Math.floor(Math.random() * statuses.length)],
    latencyUs: randomIntBetween(80, 450),
    cpuCoreWs: 2,
    cpuCoreSim: 3,
    revmDbStatus: Math.random() > 0.05 ? 'Connected' : 'Syncing',
    lastUpdate: Date.now(),
  };
}

export function generateMockFunnel(): OpportunityFunnel {
  const totalDetected = randomIntBetween(5000, 15000);
  const simulated = Math.floor(totalDetected * randomBetween(0.15, 0.25));
  const executed = Math.floor(simulated * randomBetween(0.3, 0.6));
  const reverted = Math.floor(executed * randomBetween(0.05, 0.15));
  const avgProfitEth = randomBetween(0.001, 0.05);
  const netProfitEth = executed * avgProfitEth;
  const ethPrice = 3500;
  const gasCostsEth = executed * 0.0002;
  
  return {
    totalDetected,
    simulated,
    executed,
    reverted,
    netProfitEth: netProfitEth - gasCostsEth,
    netProfitUsd: (netProfitEth - gasCostsEth) * ethPrice,
    change24h: randomBetween(-15, 25),
  };
}

export function generateMockRelays(): RelayStats[] {
  return RELAYS.map((provider, idx) => ({
    provider,
    priority: idx + 1,
    latencyMs: idx === 0 ? randomBetween(12, 45) : randomBetween(25, 120),
    successRate: idx === 0 ? randomBetween(97, 99.9) : randomBetween(94, 99),
    totalRequests: randomIntBetween(10000, 50000),
    isActive: idx === 0,
  }));
}

export function generateLatencySample(): LatencySample {
  const marketDataParsing = randomIntBetween(5, 25);
  const revmSimulation = randomIntBetween(40, 180);
  const opportunityDetection = randomIntBetween(10, 50);
  const rpcSubmission = randomIntBetween(15, 80);
  
  return {
    timestamp: Date.now(),
    marketDataParsing,
    revmSimulation,
    opportunityDetection,
    rpcSubmission,
    total: marketDataParsing + revmSimulation + opportunityDetection + rpcSubmission,
  };
}

export function generateMockTrade(): TradeExecution {
  const borrowToken = TOKENS[Math.floor(Math.random() * TOKENS.length)];
  const borrowAmount = randomIntBetween(10000, 1000000);
  const estimatedProfit = randomBetween(0.001, 0.5);
  const status = STATUSES[Math.floor(Math.random() * STATUSES.length)];
  const relay = RELAYS[Math.floor(Math.random() * RELAYS.length)];
  const gasUsed = randomIntBetween(150000, 350000);
  const gasPriceGwei = randomBetween(20, 150);
  const actualProfit = status === 'Executed' ? estimatedProfit * randomBetween(0.7, 1.1) : 0;
  
  return {
    id: `trade-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
    timestamp: Date.now(),
    route: ROUTES[Math.floor(Math.random() * ROUTES.length)],
    borrowAmount: borrowAmount.toLocaleString(),
    borrowToken,
    estimatedProfit: estimatedProfit.toFixed(6),
    estimatedProfitUsd: estimatedProfit * 3500,
    actualProfit: actualProfit.toFixed(8),
    actualProfitEth: actualProfit,
    gasUsed,
    gasPriceGwei: Number(gasPriceGwei.toFixed(2)),
    relay,
    status,
    txHash: generateTxHash(),
    revertReason: status === 'Reverted' ? 'Unprofitable after state update' : undefined,
  };
}

export function generateInitialState(): DashboardState {
  const latencyHistory: LatencySample[] = [];
  const now = Date.now();
  
  for (let i = 100; i >= 0; i--) {
    const sample = generateLatencySample();
    sample.timestamp = now - i * 1000;
    latencyHistory.push(sample);
  }
  
  const recentTrades: TradeExecution[] = [];
  for (let i = 0; i < 50; i++) {
    const trade = generateMockTrade();
    trade.timestamp = now - i * randomIntBetween(500, 5000);
    recentTrades.push(trade);
  }
  
  return {
    engine: generateMockEngine(),
    funnel: generateMockFunnel(),
    relays: generateMockRelays(),
    latencyHistory,
    recentTrades,
    isShadowMode: true,
    minProfitWei: '1000000000000000',
  };
}

export function formatNumber(num: number, decimals = 2): string {
  if (num >= 1_000_000_000) return (num / 1_000_000_000).toFixed(decimals) + 'B';
  if (num >= 1_000_000) return (num / 1_000_000).toFixed(decimals) + 'M';
  if (num >= 1_000) return (num / 1_000).toFixed(decimals) + 'K';
  return num.toFixed(decimals);
}

export function formatTimestamp(ts: number): string {
  const date = new Date(ts);
  const hours = date.getHours().toString().padStart(2, '0');
  const minutes = date.getMinutes().toString().padStart(2, '0');
  const seconds = date.getSeconds().toString().padStart(2, '0');
  const ms = date.getMilliseconds().toString().padStart(3, '0');
  return `${hours}:${minutes}:${seconds}.${ms}`;
}

export function formatGwei(wei: string): string {
  const gwei = parseFloat(wei) / 1e9;
  return gwei.toFixed(2);
}

export function getStatusColor(status: EngineStatus): string {
  switch (status) {
    case 'Active': return 'text-cyber-green';
    case 'Idle': return 'text-cyber-orange';
    case 'Paused': return 'text-cyber-muted';
    case 'Error': return 'text-cyber-red';
  }
}

export function getRelayColor(provider: RelayProvider): string {
  switch (provider) {
    case 'BloxRoute': return 'text-cyber-cyan';
    case 'Flashbots': return 'text-cyber-green';
    case 'MEV-Blocker': return 'text-cyber-orange';
    case 'Public': return 'text-cyber-red';
  }
}

export function getStatusBadgeColor(status: TradeStatus): string {
  switch (status) {
    case 'Executed': return 'bg-cyber-green/20 text-cyber-green border-cyber-green';
    case 'Reverted': return 'bg-cyber-red/20 text-cyber-red border-cyber-red';
    case 'Dropped': return 'bg-cyber-orange/20 text-cyber-orange border-cyber-orange';
    case 'Pending': return 'bg-cyber-cyan/20 text-cyber-cyan border-cyber-cyan';
  }
}
