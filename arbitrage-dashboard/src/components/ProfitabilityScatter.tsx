import { 
  ScatterChart, 
  Scatter, 
  XAxis, 
  YAxis, 
  ZAxis, 
  Tooltip, 
  ResponsiveContainer,
  Cell,
  ReferenceLine,
} from 'recharts';
import type { TradeExecution } from '../types';
import { formatTimestamp } from '../utils/mockData';

interface ProfitabilityScatterProps {
  trades: TradeExecution[];
}

export function ProfitabilityScatter({ trades }: ProfitabilityScatterProps) {
  const data = trades
    .filter(t => t.status === 'Executed')
    .map(t => ({
      gasPrice: t.gasPriceGwei,
      profit: t.actualProfitEth * 1000, // Convert to milli-ETH for better visualization
      size: Math.abs(t.actualProfitEth) * 50000 + 50,
      hash: t.txHash.slice(0, 8),
      route: t.route.slice(0, 20),
      timestamp: t.timestamp,
    }));

  const profitableTrades = data.filter(d => d.profit > 0);
  const unprofitableTrades = data.filter(d => d.profit <= 0);

  const avgProfit = profitableTrades.length > 0 
    ? profitableTrades.reduce((sum, t) => sum + t.profit, 0) / profitableTrades.length 
    : 0;

  const sweetSpot = data.reduce((best, t) => 
    t.profit > best.profit ? t : best, data[0] || { gasPrice: 50, profit: 0 });

  return (
    <div className="bg-cyber-panel border border-cyber-border rounded-lg p-4">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-sm font-semibold text-cyber-text tracking-wider">
          GAS vs PROFITABILITY
        </h2>
        <div className="flex gap-4 text-xs">
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-green" />
            Profitable
          </span>
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-red" />
            Loss
          </span>
          <span className="text-cyber-muted">
            Sweet Spot: {sweetSpot.gasPrice?.toFixed(0)} gwei
          </span>
        </div>
      </div>
      
      <div className="h-56">
        <ResponsiveContainer width="100%" height="100%">
          <ScatterChart margin={{ top: 10, right: 10, left: -20, bottom: -10 }}>
            <XAxis 
              type="number" 
              dataKey="gasPrice" 
              name="Gas Price"
              stroke="#4a4e58"
              tick={{ fontSize: 10, fill: '#4a4e58' }}
              axisLine={{ stroke: '#2a2e38' }}
              tickFormatter={(v) => `${v}`}
              label={{ value: 'Gas Price (Gwei)', position: 'bottom', offset: 0, fill: '#4a4e58', fontSize: 10 }}
              domain={[0, 200]}
            />
            <YAxis 
              type="number" 
              dataKey="profit" 
              name="Profit"
              stroke="#4a4e58"
              tick={{ fontSize: 10, fill: '#4a4e58' }}
              axisLine={{ stroke: '#2a2e38' }}
              tickFormatter={(v) => `${v.toFixed(3)}`}
              label={{ value: 'Profit (mETH)', angle: -90, position: 'insideLeft', fill: '#4a4e58', fontSize: 10 }}
            />
            <ZAxis 
              type="number" 
              dataKey="size" 
              range={[20, 200]} 
              name="Size"
            />
            
            <Tooltip
              contentStyle={{ 
                backgroundColor: '#1a1d25', 
                border: '1px solid #2a2e38',
                borderRadius: '4px',
                fontSize: '11px',
                fontFamily: 'JetBrains Mono, monospace',
              }}
              formatter={(value: number, name: string) => {
                if (name === 'profit') return [`${value.toFixed(4)} mETH`, 'Profit'];
                if (name === 'gasPrice') return [`${value.toFixed(1)} Gwei`, 'Gas Price'];
                return [value, name];
              }}
              labelFormatter={(label, payload) => {
                if (payload && payload[0]) {
                  const d = payload[0].payload;
                  return `${d.hash}... | ${formatTimestamp(d.timestamp).slice(0, 12)}`;
                }
                return label;
              }}
              content={({ active, payload }) => {
                if (active && payload && payload[0]) {
                  const data = payload[0].payload;
                  return (
                    <div className="bg-cyber-dark border border-cyber-border rounded px-3 py-2">
                      <p className="text-cyber-text font-mono text-xs">{data.route}...</p>
                      <p className="text-cyber-cyan text-xs">Gas: {data.gasPrice.toFixed(1)} Gwei</p>
                      <p className={data.profit >= 0 ? 'text-cyber-green text-xs' : 'text-cyber-red text-xs'}>
                        Profit: {data.profit.toFixed(4)} mETH
                      </p>
                    </div>
                  );
                }
                return null;
              }}
            />
            
            <ReferenceLine y={0} stroke="#ff4757" strokeDasharray="3 3" />
            
            <Scatter name="Profitable" data={profitableTrades}>
              {profitableTrades.map((_, index) => (
                <Cell 
                  key={`prof-${index}`}
                  fill="#00ff9d"
                  fillOpacity={0.6}
                  stroke="#00ff9d"
                />
              ))}
            </Scatter>
            
            <Scatter name="Unprofitable" data={unprofitableTrades}>
              {unprofitableTrades.map((_, index) => (
                <Cell 
                  key={`unprof-${index}`}
                  fill="#ff4757"
                  fillOpacity={0.6}
                  stroke="#ff4757"
                />
              ))}
            </Scatter>
          </ScatterChart>
        </ResponsiveContainer>
      </div>

      <div className="mt-3 grid grid-cols-3 gap-2 text-xs">
        <div className="p-2 bg-cyber-dark/50 rounded border border-cyber-border">
          <span className="text-cyber-muted">Avg Profit</span>
          <div className="text-cyber-green font-mono font-medium">
            {(avgProfit).toFixed(4)} mETH
          </div>
        </div>
        <div className="p-2 bg-cyber-dark/50 rounded border border-cyber-border">
          <span className="text-cyber-muted">Sweet Spot</span>
          <div className="text-cyber-cyan font-mono font-medium">
            {sweetSpot.gasPrice?.toFixed(0)} Gwei
          </div>
        </div>
        <div className="p-2 bg-cyber-dark/50 rounded border border-cyber-border">
          <span className="text-cyber-muted">Win Rate</span>
          <div className="text-cyber-text font-mono font-medium">
            {((profitableTrades.length / Math.max(data.length, 1)) * 100).toFixed(0)}%
          </div>
        </div>
      </div>
    </div>
  );
}
