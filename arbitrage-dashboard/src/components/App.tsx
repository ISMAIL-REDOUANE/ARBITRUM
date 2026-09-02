import { useState, useEffect, useCallback } from 'react';
import { DashboardState } from '../types';
import { 
  generateInitialState, 
  generateMockEngine, 
  generateMockFunnel, 
  generateMockRelays,
  generateLatencySample,
  generateMockTrade,
} from '../utils/mockData';
import { MetricCard } from './MetricCard';
import { LatencyChart } from './LatencyChart';
import { TradeStream } from './TradeStream';
import { RelayPanel } from './RelayPanel';
import { ControlPanel } from './ControlPanel';
import { ProfitabilityScatter } from './ProfitabilityScatter';
import { Activity, TrendingUp, Zap, Radio } from 'lucide-react';

export function App() {
  const [state, setState] = useState<DashboardState>(() => generateInitialState());

  const updateState = useCallback(() => {
    setState(prev => {
      const newEngine = generateMockEngine();
      const newFunnel = generateMockFunnel();
      const newRelays = generateMockRelays();
      
      const newLatencySample = generateLatencySample();
      const latencyHistory = [...prev.latencyHistory.slice(1), newLatencySample];
      
      let recentTrades = prev.recentTrades;
      if (Math.random() > 0.6) {
        recentTrades = [generateMockTrade(), ...prev.recentTrades.slice(0, 49)];
      }
      
      return {
        ...prev,
        engine: newEngine,
        funnel: newFunnel,
        relays: newRelays,
        latencyHistory,
        recentTrades,
      };
    });
  }, []);

  useEffect(() => {
    const interval = setInterval(updateState, 500);
    return () => clearInterval(interval);
  }, [updateState]);

  return (
    <div className="min-h-screen bg-cyber-black text-cyber-text font-mono">
      {/* Header */}
      <header className="border-b border-cyber-border bg-cyber-dark px-4 py-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-4">
            <div className="flex items-center gap-2">
              <Zap className="w-6 h-6 text-cyber-green" />
              <h1 className="text-xl font-bold tracking-tight">
                MEV <span className="text-cyber-green">ARBITRAGE</span> ENGINE
              </h1>
            </div>
            <span className="text-xs text-cyber-muted">
              v0.1.0 | ARBITRUM MAINNET
            </span>
          </div>
          
          <div className="flex items-center gap-4">
            {state.isShadowMode && (
              <span className="px-3 py-1 text-xs font-bold border border-cyber-orange text-cyber-orange rounded">
                SHADOW MODE
              </span>
            )}
            <span className="text-xs text-cyber-muted">
              {new Date().toLocaleTimeString()}
            </span>
          </div>
        </div>
      </header>

      {/* Metrics Bar */}
      <div className="grid grid-cols-4 gap-4 p-4 border-b border-cyber-border">
        <MetricCard
          title="ENGINE STATUS"
          icon={<Activity className="w-4 h-4" />}
          accent="green"
        >
          <div className="flex items-center gap-2">
            <span className={`text-lg font-bold ${
              state.engine.status === 'Active' ? 'text-cyber-green' : 'text-cyber-orange'
            }`}>
              {state.engine.status}
            </span>
            <span className={`px-2 py-0.5 text-xs font-mono rounded ${
              state.engine.latencyUs < 100 
                ? 'bg-cyber-green/20 text-cyber-green' 
                : 'bg-cyber-cyan/20 text-cyber-cyan'
            }`}>
              {state.engine.latencyUs} µs
            </span>
          </div>
          <div className="mt-2 text-xs text-cyber-muted">
            <span>Core 2: WS </span>
            <span className="text-cyber-cyan">Core 3: Sim</span>
          </div>
        </MetricCard>

        <MetricCard
          title="NET ARBITRAGE PROFIT"
          icon={<TrendingUp className="w-4 h-4" />}
          accent="green"
        >
          <div className="flex items-baseline gap-2">
            <span className="text-2xl font-bold text-cyber-green">
              {state.funnel.netProfitEth.toFixed(4)} ETH
            </span>
            <span className="text-sm text-cyber-muted">
              ${state.funnel.netProfitUsd.toFixed(2)}
            </span>
          </div>
          <div className={`mt-1 text-xs ${
            state.funnel.change24h >= 0 ? 'text-cyber-green' : 'text-cyber-red'
          }`}>
            {state.funnel.change24h >= 0 ? '+' : ''}{state.funnel.change24h.toFixed(1)}% 24h
          </div>
        </MetricCard>

        <MetricCard
          title="OPPORTUNITY FUNNEL"
          icon={<Zap className="w-4 h-4" />}
          accent="cyan"
        >
          <div className="space-y-1">
            <div className="flex justify-between text-xs">
              <span className="text-cyber-muted">Detected</span>
              <span className="text-cyber-text">{state.funnel.totalDetected.toLocaleString()}</span>
            </div>
            <div className="h-2 bg-cyber-panel rounded overflow-hidden flex">
              <div 
                className="bg-cyber-cyan/60 h-full" 
                style={{ width: `${(state.funnel.simulated / state.funnel.totalDetected) * 100}%` }}
              />
              <div 
                className="bg-cyber-green/60 h-full" 
                style={{ width: `${(state.funnel.executed / state.funnel.totalDetected) * 100}%` }}
              />
            </div>
            <div className="flex justify-between text-xs">
              <span className="text-cyber-green">Executed: {state.funnel.executed}</span>
              <span className="text-cyber-red">Reverted: {state.funnel.reverted}</span>
            </div>
          </div>
        </MetricCard>

        <MetricCard
          title="PRIVATE RPC STATUS"
          icon={<Radio className="w-4 h-4" />}
          accent="green"
        >
          <div className="flex items-center gap-2">
            <span className="text-sm font-bold text-cyber-green">
              {state.relays.find(r => r.isActive)?.provider || 'Public'}
            </span>
            <span className="px-2 py-0.5 text-xs bg-cyber-green/20 text-cyber-green rounded">
              {state.relays.find(r => r.isActive)?.latencyMs.toFixed(0)}ms
            </span>
          </div>
          <div className="mt-1 text-xs text-cyber-muted">
            Success: {state.relays.find(r => r.isActive)?.successRate.toFixed(1)}%
          </div>
        </MetricCard>
      </div>

      {/* Main Content */}
      <div className="grid grid-cols-3 gap-4 p-4">
        {/* Left Column - Charts */}
        <div className="col-span-2 space-y-4">
          <LatencyChart data={state.latencyHistory} />
          <ProfitabilityScatter trades={state.recentTrades} />
          <TradeStream trades={state.recentTrades} />
        </div>

        {/* Right Column - Panels */}
        <div className="space-y-4">
          <RelayPanel relays={state.relays} />
          <ControlPanel 
            isShadowMode={state.isShadowMode}
            minProfitWei={state.minProfitWei}
            onToggleShadowMode={() => setState(s => ({ ...s, isShadowMode: !s.isShadowMode }))}
          />
        </div>
      </div>
    </div>
  );
}
