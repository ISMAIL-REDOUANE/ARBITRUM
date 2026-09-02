export type EngineStatus = 'Active' | 'Idle' | 'Paused' | 'Error';

export type RelayProvider = 'BloxRoute' | 'Flashbots' | 'MEV-Blocker' | 'Public';

export type TradeStatus = 'Executed' | 'Reverted' | 'Dropped' | 'Pending';

export interface LatencySample {
  timestamp: number;
  marketDataParsing: number;
  revmSimulation: number;
  opportunityDetection: number;
  rpcSubmission: number;
  total: number;
}

export interface TradeExecution {
  id: string;
  timestamp: number;
  route: string;
  borrowAmount: string;
  borrowToken: string;
  estimatedProfit: string;
  estimatedProfitUsd: number;
  actualProfit: string;
  actualProfitEth: number;
  gasUsed: number;
  gasPriceGwei: number;
  relay: RelayProvider;
  status: TradeStatus;
  txHash: string;
  revertReason?: string;
}

export interface OpportunityFunnel {
  totalDetected: number;
  simulated: number;
  executed: number;
  reverted: number;
  netProfitEth: number;
  netProfitUsd: number;
  change24h: number;
}

export interface RelayStats {
  provider: RelayProvider;
  priority: number;
  latencyMs: number;
  successRate: number;
  totalRequests: number;
  isActive: boolean;
}

export interface EngineMetrics {
  status: EngineStatus;
  latencyUs: number;
  cpuCoreWs: number;
  cpuCoreSim: number;
  revmDbStatus: 'Connected' | 'Syncing' | 'Error';
  lastUpdate: number;
}

export interface DashboardState {
  engine: EngineMetrics;
  funnel: OpportunityFunnel;
  relays: RelayStats[];
  latencyHistory: LatencySample[];
  recentTrades: TradeExecution[];
  isShadowMode: boolean;
  minProfitWei: string;
}
