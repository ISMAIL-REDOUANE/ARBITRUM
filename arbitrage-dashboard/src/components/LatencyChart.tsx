import { 
  AreaChart, 
  Area, 
  XAxis, 
  YAxis, 
  Tooltip, 
  ResponsiveContainer,
} from 'recharts';
import type { LatencySample } from '../types';
import { formatTimestamp } from '../utils/mockData';

interface LatencyChartProps {
  data: LatencySample[];
}

export function LatencyChart({ data }: LatencyChartProps) {
  const chartData = data.map(d => ({
    ...d,
    time: formatTimestamp(d.timestamp).split('.')[1] || '000',
  }));

  return (
    <div className="bg-cyber-panel border border-cyber-border rounded-lg p-4">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-sm font-semibold text-cyber-text tracking-wider">
          LATENCY BREAKDOWN
        </h2>
        <div className="flex gap-4 text-xs">
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-cyan" />
            Market Data
          </span>
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-green" />
            REVM Sim
          </span>
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-orange" />
            Detection
          </span>
          <span className="flex items-center gap-1">
            <span className="w-2 h-2 rounded-full bg-cyber-red" />
            RPC Submit
          </span>
        </div>
      </div>
      
      <div className="h-64">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={chartData} margin={{ top: 5, right: 5, left: 5, bottom: 5 }}>
            <defs>
              <linearGradient id="colorMarket" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="#00d4ff" stopOpacity={0.3}/>
                <stop offset="95%" stopColor="#00d4ff" stopOpacity={0}/>
              </linearGradient>
              <linearGradient id="colorRevm" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="#00ff9d" stopOpacity={0.3}/>
                <stop offset="95%" stopColor="#00ff9d" stopOpacity={0}/>
              </linearGradient>
              <linearGradient id="colorDetection" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="#ff9d00" stopOpacity={0.3}/>
                <stop offset="95%" stopColor="#ff9d00" stopOpacity={0}/>
              </linearGradient>
              <linearGradient id="colorRpc" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="#ff4757" stopOpacity={0.3}/>
                <stop offset="95%" stopColor="#ff4757" stopOpacity={0}/>
              </linearGradient>
            </defs>
            
            <XAxis 
              dataKey="time" 
              stroke="#4a4e58" 
              tick={{ fontSize: 10, fill: '#4a4e58' }}
              axisLine={{ stroke: '#2a2e38' }}
            />
            <YAxis 
              stroke="#4a4e58" 
              tick={{ fontSize: 10, fill: '#4a4e58' }}
              axisLine={{ stroke: '#2a2e38' }}
              tickFormatter={(v) => `${v}µs`}
            />
            
            <Tooltip 
              contentStyle={{ 
                backgroundColor: '#1a1d25', 
                border: '1px solid #2a2e38',
                borderRadius: '4px',
                fontSize: '11px',
                fontFamily: 'JetBrains Mono, monospace',
              }}
              labelFormatter={(v) => `Time: ${v}`}
              formatter={(value: number, name: string) => [`${value}µs`, name]}
              labelStyle={{ color: '#e0e2e8' }}
            />
            
            <Area
              type="monotone"
              dataKey="marketDataParsing"
              stackId="1"
              stroke="#00d4ff"
              fill="url(#colorMarket)"
              name="Market Data"
            />
            <Area
              type="monotone"
              dataKey="revmSimulation"
              stackId="1"
              stroke="#00ff9d"
              fill="url(#colorRevm)"
              name="REVM Sim"
            />
            <Area
              type="monotone"
              dataKey="opportunityDetection"
              stackId="1"
              stroke="#ff9d00"
              fill="url(#colorDetection)"
              name="Detection"
            />
            <Area
              type="monotone"
              dataKey="rpcSubmission"
              stackId="1"
              stroke="#ff4757"
              fill="url(#colorRpc)"
              name="RPC Submit"
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
