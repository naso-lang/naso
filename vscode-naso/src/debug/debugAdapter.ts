import * as vscode from 'vscode';
import {
    DebugAdapter,
    DebugProtocol,
    DebugSession,
    InitializedEvent,
    StoppedEvent,
    TerminatedEvent,
    ThreadEvent,
    OutputEvent,
    BreakpointEvent,
    CapabilitiesEvent,
    Variable,
    Scope,
    Source,
    StackFrame,
    Breakpoint
} from 'vscode-debugadapter';
import { DebugProtocol as DebugProtocolTypes } from 'vscode-debugprotocol';

interface LaunchRequestArguments extends DebugProtocolTypes.LaunchRequestArguments {
    program: string;
    args?: string[];
    cwd?: string;
    env?: { [key: string]: string };
    stopAtEntry?: boolean;
}

interface StackFrameInfo {
    id: number;
    name: string;
    file: string;
    line: number;
    column: number;
    source: Source;
}

class NasoDebugSession extends DebugSession {
    private _gdbProcess: import('child_process').ChildProcessWithoutNullStreams | null = null;
    private _breakpoints: Map<string, Map<number, Breakpoint>> = new Map();
    private _threadId = 1;
    private _frameId = 1;
    private _configurationDone = false;
    private _program: string = '';
    private _args: string[] = [];
    private _cwd: string = '';
    private _stopAtEntry: boolean = false;

    public constructor() {
        super();
        this.setDebuggerLinesStartAt1(true);
        this.setDebuggerColumnsStartAt1(true);
    }

    protected initializeRequest(
        response: DebugProtocolTypes.InitializeResponse,
        args: DebugProtocolTypes.InitializeRequestArguments
    ): void {
        response.body = response.body || {};
        response.body.supportsConfigurationDoneRequest = true;
        response.body.supportsFunctionBreakpoints = true;
        response.body.supportsConditionalBreakpoints = true;
        response.body.supportsHitConditionalBreakpoints = true;
        response.body.supportsEvaluateForHovers = true;
        response.body.supportsStepBack = false;
        response.body.supportsSetVariable = true;
        response.body.supportsGotoTargetsRequest = true;
        response.body.supportsModulesRequest = true;
        response.body.additionalModuleColumns = [
            { attributeName: 'address', label: 'Address', type: 'string', width: 12 },
            { attributeName: 'symbols', label: 'Symbols', type: 'boolean', width: 8 }
        ];
        response.body.supportsCompletionsRequest = true;
        response.body.supportsExceptionInfoRequest = true;
        response.body.supportsTerminateThreadsRequest = true;
        response.body.supportsSetExpression = true;
        response.body.supportsValueFormattingOptions = true;
        response.body.supportsExceptionOptions = true;
        response.body.supportsDataBreakpoints = true;
        response.body.supportsReadMemoryRequest = true;
        response.body.supportsDisassembleRequest = true;
        response.body.supportsCancelRequest = true;
        response.body.supportsBreakpointLocationsRequest = true;
        response.body.supportsClipboardContext = true;
        response.body.supportsSteppingGranularity = true;
        response.body.supportsInstructionBreakpoints = true;
        response.body.supportsExceptionFilterOptions = true;
        response.body.supportsSingleThreadExecutionRequests = true;

        this.sendResponse(response);
    }

    protected launchRequest(
        response: DebugProtocolTypes.LaunchResponse,
        args: LaunchRequestArguments
    ): void {
        this._program = args.program;
        this._args = args.args || [];
        this._cwd = args.cwd || process.cwd();
        this._stopAtEntry = args.stopAtEntry || false;

        const gdbPath = vscode.workspace.getConfiguration('naso.debug').get<string>('gdbPath', 'gdb');
        
        this._gdbProcess = this.spawnGdb(gdbPath);

        if (!this._gdbProcess) {
            this.sendErrorResponse(response, 0, 'Failed to start GDB');
            return;
        }

        this.setupGdbEventHandlers();
        this.sendResponse(response);
    }

    private spawnGdb(gdbPath: string): import('child_process').ChildProcessWithoutNullStreams | null {
        const { spawn } = require('child_process');
        
        const gdbArgs = [
            '--interpreter=mi3',
            '--quiet',
            this._program
        ];

        try {
            const gdbProcess = spawn(gdbPath, gdbArgs, {
                cwd: this._cwd,
                env: { ...process.env, ...this._getEnv() }
            });

            return gdbProcess;
        } catch (error) {
            this.sendEvent(new OutputEvent(`Failed to spawn GDB: ${error}\n`, 'stderr'));
            return null;
        }
    }

    private _getEnv(): { [key: string]: string } {
        return {};
    }

    private setupGdbEventHandlers(): void {
        if (!this._gdbProcess) return;

        this._gdbProcess.stdout.on('data', (data: Buffer) => {
            const output = data.toString();
            this.handleGdbOutput(output);
        });

        this._gdbProcess.stderr.on('data', (data: Buffer) => {
            const output = data.toString();
            this.sendEvent(new OutputEvent(output, 'stderr'));
        });

        this._gdbProcess.on('close', (code: number | null) => {
            this.sendEvent(new TerminatedEvent());
        });

        this._gdbProcess.on('error', (error: Error) => {
            this.sendEvent(new OutputEvent(`GDB error: ${error.message}\n`, 'stderr'));
        });
    }

    private handleGdbOutput(output: string): void {
        const lines = output.split('\n');
        
        for (const line of lines) {
            if (!line.trim()) continue;
            
            if (line.startsWith('^')) {
                this.handleGdbResultRecord(line);
            } else if (line.startsWith('*') || line.startsWith('=') || line.startsWith('~') || line.startsWith('@')) {
                this.handleGdbAsyncRecord(line);
            }
        }
    }

    private handleGdbResultRecord(line: string): void {
        // Handle GDB MI result records
        const match = line.match(/^\^([^,]+),?(.*)$/);
        if (!match) return;

        const resultClass = match[1];
        const results = match[2];

        if (resultClass === 'done') {
            if (results.includes('thread-id')) {
                // Thread created
            }
        } else if (resultClass === 'error') {
            this.sendEvent(new OutputEvent(`GDB error: ${results}\n`, 'stderr'));
        }
    }

    private handleGdbAsyncRecord(line: string): void {
        if (line.startsWith('*stopped,')) {
            this.handleStoppedEvent(line);
        } else if (line.startsWith('*running,')) {
            this.sendEvent(new ThreadEvent('started', this._threadId));
        } else if (line.startsWith('=thread-group-added,')) {
            // Thread group added
        } else if (line.startsWith('=thread-created,')) {
            // Thread created
        } else if (line.startsWith('=library-loaded,')) {
            // Library loaded
        } else if (line.startsWith('~')) {
            this.sendEvent(new OutputEvent(line.slice(1) + '\n', 'stdout'));
        } else if (line.startsWith('@')) {
            this.sendEvent(new OutputEvent(line.slice(1) + '\n', 'stdout'));
        }
    }

    private handleStoppedEvent(line: string): void {
        const reasonMatch = line.match(/reason="([^"]+)"/);
        const frameMatch = line.match(/frame=\{([^}]+)\}/);
        
        let reason = 'breakpoint';
        if (reasonMatch) {
            const r = reasonMatch[1];
            if (r === 'end-stepping-range') reason = 'step';
            else if (r === 'breakpoint-hit') reason = 'breakpoint';
            else if (r === 'signal-received') reason = 'exception';
        }

        let file = '';
        let line = 1;
        let func = 'unknown';

        if (frameMatch) {
            const frame = frameMatch[1];
            const fileMatch = frame.match(/file="([^"]+)"/);
            const lineMatch = frame.match(/line="(\d+)"/);
            const funcMatch = frame.match(/func="([^"]+)"/);
            
            if (fileMatch) file = fileMatch[1];
            if (lineMatch) line = parseInt(lineMatch[1], 10);
            if (funcMatch) func = funcMatch[1];
        }

        this.sendEvent(new StoppedEvent(reason, this._threadId));
        
        if (file) {
            const stackFrame: StackFrameInfo = {
                id: this._frameId++,
                name: func,
                file,
                line,
                column: 1,
                source: new Source(file, file)
            };
        }
    }

    protected configurationDoneRequest(
        response: DebugProtocolTypes.ConfigurationDoneResponse,
        args: DebugProtocolTypes.ConfigurationDoneArguments
    ): void {
        this._configurationDone = true;
        
        // Set breakpoints
        this.setBreakpointsInGdb();
        
        // Start debugging
        this.sendGdbCommand('-exec-run');
        
        this.sendResponse(response);
    }

    private setBreakpointsInGdb(): void {
        for (const [file, breakpoints] of this._breakpoints.entries()) {
            for (const [line, bp] of breakpoints.entries()) {
                if (bp.verified) {
                    this.sendGdbCommand(`-break-insert -f "${file}:${line}"`);
                }
            }
        }
    }

    private sendGdbCommand(command: string): void {
        if (this._gdbProcess && this._gdbProcess.stdin) {
            this._gdbProcess.stdin.write(command + '\n');
        }
    }

    protected setBreakPointsRequest(
        response: DebugProtocolTypes.SetBreakpointsResponse,
        args: DebugProtocolTypes.SetBreakpointsArguments
    ): void {
        const path = args.source.path || '';
        const newBreakpoints = new Map<number, Breakpoint>();

        for (const bp of args.breakpoints || []) {
            const verified = !!path && bp.line > 0;
            const newBp: Breakpoint = {
                verified,
                line: bp.line,
                id: bp.id
            };
            newBreakpoints.set(bp.line, newBp);
        }

        this._breakpoints.set(path, newBreakpoints);

        response.body = {
            breakpoints: Array.from(newBreakpoints.values())
        };

        if (this._configurationDone) {
            this.setBreakpointsInGdb();
        }

        this.sendResponse(response);
    }

    protected setFunctionBreakPointsRequest(
        response: DebugProtocolTypes.SetFunctionBreakpointsResponse,
        args: DebugProtocolTypes.SetFunctionBreakpointsArguments
    ): void {
        const breakpoints: DebugProtocolTypes.Breakpoint[] = [];
        
        for (const fb of args.breakpoints || []) {
            this.sendGdbCommand(`-break-insert -f ${fb.name}`);
            breakpoints.push({ verified: true });
        }

        response.body = { breakpoints };
        this.sendResponse(response);
    }

    protected threadsRequest(
        response: DebugProtocolTypes.ThreadsResponse,
        args: DebugProtocolTypes.ThreadsArguments
    ): void {
        this.sendGdbCommand('-thread-info');
        
        response.body = {
            threads: [
                { id: this._threadId, name: 'Main Thread' }
            ]
        };
        this.sendResponse(response);
    }

    protected stackTraceRequest(
        response: DebugProtocolTypes.StackTraceResponse,
        args: DebugProtocolTypes.StackTraceArguments
    ): void {
        this.sendGdbCommand('-stack-list-frames');
        
        response.body = {
            stackFrames: [
                {
                    id: 1,
                    name: 'main',
                    source: { path: this._program, name: 'main.naso' },
                    line: 1,
                    column: 1
                }
            ]
        };
        this.sendResponse(response);
    }

    protected scopesRequest(
        response: DebugProtocolTypes.ScopesResponse,
        args: DebugProtocolTypes.ScopesArguments
    ): void {
        response.body = {
            scopes: [
                {
                    name: 'Local',
                    variablesReference: 1,
                    expensive: false
                },
                {
                    name: 'Global',
                    variablesReference: 2,
                    expensive: true
                }
            ]
        };
        this.sendResponse(response);
    }

    protected variablesRequest(
        response: DebugProtocolTypes.VariablesResponse,
        args: DebugProtocolTypes.VariablesArguments
    ): void {
        const variables: Variable[] = [];
        
        if (args.variablesReference === 1) {
            // Local variables - would query GDB
            variables.push(
                { name: 'x', value: '42', type: 'int', variablesReference: 0 },
                { name: 'y', value: '"hello"', type: 'string', variablesReference: 0 }
            );
        } else if (args.variablesReference === 2) {
            // Global variables
            variables.push(
                { name: 'PI', value: '3.14159', type: 'float', variablesReference: 0 }
            );
        }

        response.body = { variables };
        this.sendResponse(response);
    }

    protected evaluateRequest(
        response: DebugProtocolTypes.EvaluateResponse,
        args: DebugProtocolTypes.EvaluateArguments
    ): void {
        const expression = args.expression;
        
        if (args.context === 'hover') {
            // For hover evaluation, just return the expression as value
            response.body = {
                result: expression,
                variablesReference: 0
            };
        } else {
            this.sendGdbCommand(`-data-evaluate-expression "${expression}"`);
            response.body = {
                result: expression,
                variablesReference: 0
            };
        }
        
        this.sendResponse(response);
    }

    protected continueRequest(
        response: DebugProtocolTypes.ContinueResponse,
        args: DebugProtocolTypes.ContinueArguments
    ): void {
        this.sendGdbCommand('-exec-continue');
        this.sendResponse(response);
    }

    protected nextRequest(
        response: DebugProtocolTypes.NextResponse,
        args: DebugProtocolTypes.NextArguments
    ): void {
        this.sendGdbCommand('-exec-next');
        this.sendResponse(response);
    }

    protected stepInRequest(
        response: DebugProtocolTypes.StepInResponse,
        args: DebugProtocolTypes.StepInArguments
    ): void {
        this.sendGdbCommand('-exec-step');
        this.sendResponse(response);
    }

    protected stepOutRequest(
        response: DebugProtocolTypes.StepOutResponse,
        args: DebugProtocolTypes.StepOutArguments
    ): void {
        this.sendGdbCommand('-exec-finish');
        this.sendResponse(response);
    }

    protected pauseRequest(
        response: DebugProtocolTypes.PauseResponse,
        args: DebugProtocolTypes.PauseArguments
    ): void {
        this.sendGdbCommand('-exec-interrupt');
        this.sendResponse(response);
    }

    protected disconnectRequest(
        response: DebugProtocolTypes.DisconnectResponse,
        args: DebugProtocolTypes.DisconnectArguments
    ): void {
        if (this._gdbProcess) {
            this._gdbProcess.kill('SIGTERM');
            this._gdbProcess = null;
        }
        this.sendResponse(response);
    }

    protected terminateRequest(
        response: DebugProtocolTypes.TerminateResponse,
        args: DebugProtocolTypes.TerminateArguments
    ): void {
        if (this._gdbProcess) {
            this._gdbProcess.kill('SIGTERM');
            this._gdbProcess = null;
        }
        this.sendResponse(response);
    }
}

DebugSession.run(NasoDebugSession);