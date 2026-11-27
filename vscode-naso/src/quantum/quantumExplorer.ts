import * as vscode from 'vscode';
import { QuantumCircuitParser } from './quantumParser';

export class QuantumCircuitExplorer implements vscode.TreeDataProvider<QuantumCircuitNode> {
    private _onDidChangeTreeData: vscode.EventEmitter<QuantumCircuitNode | undefined | null | void> = new vscode.EventEmitter<QuantumCircuitNode | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<QuantumCircuitNode | undefined | null | void> = this._onDidChangeTreeData.event;
    
    private circuits: Map<string, QuantumCircuitInfo> = new Map();

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    updateCircuit(filePath: string, content: string): void {
        const parser = new QuantumCircuitParser();
        const circuit = parser.parse(content);
        
        if (circuit) {
            this.circuits.set(filePath, {
                filePath,
                name: circuit.name,
                qubits: circuit.qubits,
                gates: circuit.gates,
                measurements: circuit.measurements
            });
        } else {
            this.circuits.delete(filePath);
        }
        
        this.refresh();
    }

    getTreeItem(element: QuantumCircuitNode): vscode.TreeItem {
        return element;
    }

    getChildren(element?: QuantumCircuitNode): Thenable<QuantumCircuitNode[]> {
        if (!element) {
            // Root level - show all files with quantum circuits
            const items: QuantumCircuitNode[] = [];
            
            for (const [filePath, circuit] of this.circuits.entries()) {
                const relativePath = vscode.workspace.asRelativePath(filePath);
                items.push(new QuantumCircuitNode(
                    relativePath,
                    vscode.TreeItemCollapsibleState.Collapsed,
                    'circuit',
                    filePath
                ));
            }
            
            return Promise.resolve(items);
        } else if (element.contextValue === 'circuit') {
            // Show circuit details
            const circuit = this.circuits.get(element.filePath);
            if (!circuit) return Promise.resolve([]);
            
            const items: QuantumCircuitNode[] = [];
            
            // Qubits
            items.push(new QuantumCircuitNode(
                `Qubits: ${circuit.qubits}`,
                vscode.TreeItemCollapsibleState.None,
                'qubits',
                element.filePath
            ));
            
            // Gates
            for (const gate of circuit.gates) {
                items.push(new QuantumCircuitNode(
                    `${gate.name} ${gate.qubits.join(', ')}`,
                    vscode.TreeItemCollapsibleState.None,
                    'gate',
                    element.filePath,
                    gate
                ));
            }
            
            // Measurements
            for (const measurement of circuit.measurements) {
                items.push(new QuantumCircuitNode(
                    `measure ${measurement.qubit} -> ${measurement.classicalBit}`,
                    vscode.TreeItemCollapsibleState.None,
                    'measurement',
                    element.filePath,
                    measurement
                ));
            }
            
            return Promise.resolve(items);
        }
        
        return Promise.resolve([]);
    }
}

export class QuantumCircuitNode extends vscode.TreeItem {
    constructor(
        public readonly label: string,
        public readonly collapsibleState: vscode.TreeItemCollapsibleState,
        public readonly contextValue: string,
        public readonly filePath: string,
        public readonly data?: any
    ) {
        super(label, collapsibleState);
        
        this.tooltip = `${contextValue}: ${label}`;
        
        switch (contextValue) {
            case 'circuit':
                this.iconPath = new vscode.ThemeIcon('circuit');
                break;
            case 'qubits':
                this.iconPath = new vscode.ThemeIcon('symbol-number');
                break;
            case 'gate':
                this.iconPath = new vscode.ThemeIcon('symbol-method');
                break;
            case 'measurement':
                this.iconPath = new vscode.ThemeIcon('symbol-constant');
                break;
        }
        
        if (contextValue === 'gate' && data) {
            this.command = {
                command: 'naso.quantumVisualize',
                title: 'Visualize Gate',
                arguments: [this.filePath, data]
            };
        }
    }
}

export interface QuantumCircuitInfo {
    filePath: string;
    name: string;
    qubits: number;
    gates: QuantumGate[];
    measurements: QuantumMeasurement[];
}

export interface QuantumGate {
    name: string;
    qubits: number[];
    params?: number[];
    controlled?: boolean;
    controlQubits?: number[];
}

export interface QuantumMeasurement {
    qubit: number;
    classicalBit: number;
}

export class QuantumCircuitParser {
    parse(content: string): QuantumCircuitInfo | null {
        // Parse Naso quantum circuit syntax
        const circuitMatch = content.match(/circuit\s+(\w+)\s*\{([\s\S]*?)\}/);
        if (!circuitMatch) return null;
        
        const name = circuitMatch[1];
        const body = circuitMatch[2];
        
        // Parse qubit declaration
        const qubitMatch = body.match(/qubits\s+(\d+)/);
        const qubits = qubitMatch ? parseInt(qubitMatch[1], 10) : 0;
        
        // Parse gates
        const gates: QuantumGate[] = [];
        const gateRegex = /(\w+)(?:\(([^)]+)\))?\s+([\d,\s]+)/g;
        let match;
        
        while ((match = gateRegex.exec(body)) !== null) {
            const gateName = match[1];
            const params = match[2] ? match[2].split(',').map(p => parseFloat(p.trim())) : undefined;
            const qubitStr = match[3];
            const qubits = qubitStr.split(',').map(q => parseInt(q.trim(), 10));
            
            gates.push({
                name: gateName,
                qubits,
                params
            });
        }
        
        // Parse measurements
        const measurements: QuantumMeasurement[] = [];
        const measureRegex = /measure\s+(\d+)\s*->\s*(\w+)/g;
        
        while ((match = measureRegex.exec(body)) !== null) {
            measurements.push({
                qubit: parseInt(match[1], 10),
                classicalBit: parseInt(match[2], 10)
            });
        }
        
        return {
            filePath: '',
            name,
            qubits,
            gates,
            measurements
        };
    }
}

export class QuantumVisualizationPanel {
    public static currentPanel: QuantumVisualizationPanel | undefined;
    private readonly _panel: vscode.WebviewPanel;
    private readonly _extensionUri: vscode.Uri;
    private _disposables: vscode.Disposable[] = [];

    public static createOrShow(extensionUri: vscode.Uri, circuit: QuantumCircuitInfo): void {
        const column = vscode.window.activeTextEditor ? vscode.window.activeTextEditor.viewColumn : undefined;

        if (QuantumVisualizationPanel.currentPanel) {
            QuantumVisualizationPanel.currentPanel._panel.reveal(column);
            QuantumVisualizationPanel.currentPanel.update(circuit);
            return;
        }

        const panel = vscode.window.createWebviewPanel(
            'nasoQuantumVisualization',
            `Quantum Circuit: ${circuit.name}`,
            column || vscode.ViewColumn.One,
            {
                enableScripts: true,
                localResourceRoots: [vscode.Uri.joinPath(extensionUri, 'media')],
                retainContextWhenHidden: true
            }
        );

        QuantumVisualizationPanel.currentPanel = new QuantumVisualizationPanel(panel, extensionUri, circuit);
    }

    private constructor(panel: vscode.WebviewPanel, extensionUri: vscode.Uri, circuit: QuantumCircuitInfo) {
        this._panel = panel;
        this._extensionUri = extensionUri;
        
        this._panel.webview.html = this._getHtmlForWebview(circuit);
        
        this._panel.onDidDispose(() => this.dispose(), null, this._disposables);
        
        this._panel.webview.onDidReceiveMessage(
            message => {
                switch (message.command) {
                    case 'simulate':
                        this.simulateCircuit(circuit);
                        break;
                    case 'exportOpenQASM':
                        this.exportOpenQASM(circuit);
                        break;
                }
            },
            null,
            this._disposables
        );
    }

    public update(circuit: QuantumCircuitInfo): void {
        this._panel.webview.html = this._getHtmlForWebview(circuit);
    }

    public dispose(): void {
        QuantumVisualizationPanel.currentPanel = undefined;
        this._panel.dispose();
        while (this._disposables.length) {
            const x = this._disposables.pop();
            if (x) x.dispose();
        }
    }

    private _getHtmlForWebview(circuit: QuantumCircuitInfo): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Quantum Circuit: ${circuit.name}</title>
    <style>
        body { font-family: var(--vscode-font-family); padding: 20px; color: var(--vscode-foreground); }
        .circuit { font-family: monospace; white-space: pre; line-height: 2; }
        .gate { display: inline-block; padding: 2px 8px; margin: 2px; border-radius: 4px; background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
        .gate-h { background: #ff6b6b; }
        .gate-x { background: #4ecdc4; }
        .gate-y { background: #ffe66d; }
        .gate-z { background: #95e1d3; }
        .gate-cnot { background: #f38181; }
        .gate-cz { background: #aa96da; }
        .gate-measure { background: #fcbad3; }
        .qubit-line { margin: 10px 0; }
        .controls { margin-bottom: 20px; }
        button { padding: 8px 16px; margin-right: 10px; background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: none; border-radius: 4px; cursor: pointer; }
        button:hover { opacity: 0.9; }
        .legend { margin-top: 20px; font-size: 0.9em; }
        .legend-item { display: inline-block; margin-right: 15px; }
    </style>
</head>
<body>
    <div class="controls">
        <button onclick="simulate()">▶ Simulate</button>
        <button onclick="exportOpenQASM()">📥 Export OpenQASM</button>
    </div>
    
    <div class="circuit" id="circuit">${this.renderCircuit(circuit)}</div>
    
    <div class="legend">
        <div class="legend-item"><span class="gate gate-h">H</span> Hadamard</div>
        <div class="legend-item"><span class="gate gate-x">X</span> Pauli-X</div>
        <div class="legend-item"><span class="gate gate-y">Y</span> Pauli-Y</div>
        <div class="legend-item"><span class="gate gate-z">Z</span> Pauli-Z</div>
        <div class="legend-item"><span class="gate gate-cnot">CNOT</span> Controlled-X</div>
        <div class="legend-item"><span class="gate gate-cz">CZ</span> Controlled-Z</div>
        <div class="legend-item"><span class="gate gate-measure">Measure</span> Measurement</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();
        
        function simulate() {
            vscode.postMessage({ command: 'simulate' });
        }
        
        function exportOpenQASM() {
            vscode.postMessage({ command: 'exportOpenQASM' });
        }
    </script>
</body>
</html>`;
    }

    private renderCircuit(circuit: QuantumCircuitInfo): string {
        const numQubits = circuit.qubits;
        const lines: string[] = [];
        
        // Initialize qubit lines
        for (let i = 0; i < numQubits; i++) {
            lines[i] = `q${i}: `;
        }
        
        // Add gates to lines
        for (const gate of circuit.gates) {
            const gateClass = this.getGateClass(gate.name);
            
            if (gate.controlled && gate.controlQubits) {
                // Controlled gate
                for (const controlQubit of gate.controlQubits) {
                    lines[controlQubit] += `●──`;
                }
                for (const targetQubit of gate.qubits) {
                    lines[targetQubit] += `├─[${gate.name}]─`;
                }
                // Other qubits
                for (let i = 0; i < numQubits; i++) {
                    if (!gate.controlQubits.includes(i) && !gate.qubits.includes(i)) {
                        lines[i] += `│─────`;
                    }
                }
            } else {
                // Single/multi-qubit gate
                for (const qubit of gate.qubits) {
                    const params = gate.params ? `(${gate.params.join(',')})` : '';
                    lines[qubit] += `─[${gate.name}${params}]─`;
                }
                // Other qubits
                for (let i = 0; i < numQubits; i++) {
                    if (!gate.qubits.includes(i)) {
                        lines[i] += `───────`;
                    }
                }
            }
        }
        
        // Add measurements
        for (const measurement of circuit.measurements) {
            lines[measurement.qubit] += `─[M]─`;
            for (let i = 0; i < numQubits; i++) {
                if (i !== measurement.qubit) {
                    lines[i] += `─────`;
                }
            }
        }
        
        return lines.join('\n');
    }

    private getGateClass(gateName: string): string {
        const lower = gateName.toLowerCase();
        if (lower === 'h') return 'gate-h';
        if (lower === 'x') return 'gate-x';
        if (lower === 'y') return 'gate-y';
        if (lower === 'z') return 'gate-z';
        if (lower === 'cnot' || lower === 'cx') return 'gate-cnot';
        if (lower === 'cz') return 'gate-cz';
        if (lower === 'measure' || lower === 'm') return 'gate-measure';
        return 'gate';
    }

    private simulateCircuit(circuit: QuantumCircuitInfo): void {
        vscode.window.showInformationMessage(`Simulating ${circuit.name} with ${circuit.qubits} qubits...`);
        // Would integrate with naso-gates or external simulator
    }

    private exportOpenQASM(circuit: QuantumCircuitInfo): void {
        let openqasm = 'OPENQASM 2.0;\ninclude "qelib1.inc";\n\n';
        openqasm += `qreg q[${circuit.qubits}];\n`;
        
        if (circuit.measurements.length > 0) {
            openqasm += `creg c[${circuit.measurements.length}];\n\n`;
        }
        
        for (const gate of circuit.gates) {
            const qargs = gate.qubits.map(q => `q[${q}]`).join(',');
            
            switch (gate.name.toLowerCase()) {
                case 'h':
                    openqasm += `h ${qargs};\n`;
                    break;
                case 'x':
                    openqasm += `x ${qargs};\n`;
                    break;
                case 'y':
                    openqasm += `y ${qargs};\n`;
                    break;
                case 'z':
                    openqasm += `z ${qargs};\n`;
                    break;
                case 'cnot':
                case 'cx':
                    if (gate.qubits.length === 2) {
                        openqasm += `cx q[${gate.qubits[0]}],q[${gate.qubits[1]}];\n`;
                    }
                    break;
                case 'cz':
                    if (gate.qubits.length === 2) {
                        openqasm += `cz q[${gate.qubits[0]}],q[${gate.qubits[1]}];\n`;
                    }
                    break;
                case 'rx':
                case 'ry':
                case 'rz':
                    if (gate.params && gate.params.length > 0) {
                        openqasm += `${gate.name}(${gate.params[0]}) ${qargs};\n`;
                    }
                    break;
                default:
                    openqasm += `// ${gate.name} ${qargs}\n`;
            }
        }
        
        for (const measurement of circuit.measurements) {
            openqasm += `measure q[${measurement.qubit}] -> c[${measurement.classicalBit}];\n`;
        }
        
        const uri = vscode.Uri.parse(`untitled:${circuit.name}.qasm`);
        vscode.workspace.openTextDocument({ content: openqasm, language: 'openqasm' }).then(doc => {
            vscode.window.showTextDocument(doc);
        });
    }
}