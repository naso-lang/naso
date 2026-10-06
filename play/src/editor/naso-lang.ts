// Naso Language Definition for Monaco Editor
// Registers syntax highlighting, tokenization, and language configuration

import * as monaco from 'monaco-editor';

// Naso language ID
export const NASO_LANGUAGE_ID = 'naso';

// Keywords
const keywords = [
  'fn', 'let', 'mut', 'inout', 'consume', 'reversible', 'return',
  'struct', 'enum', 'type', 'val', 'const', 'import', 'mod',
  'if', 'else', 'while', 'for', 'loop', 'break', 'continue',
  'match', 'as', 'is', 'true', 'false', 'Self',
  'Int', 'Bool', 'Float', 'Qubit', 'QRegister', 'Tensor',
  'qalloc', 'hadamard', 'cnot', 'measure', 'qfree',
  'linear_alloc', 'linear_free', 'alloc_tensor',
];

// Quantity markers
const quantityMarkers = [
  '\\\\[0\\\\]', '\\\\[1\\\\]', '\\\\[\\\\*\\\\]', '\\\\[q0\\\\]', '\\\\[q1\\\\]', '\\\\[qstar\\\\]',
];

// Type keywords
const typeKeywords = [
  'Int', 'Bool', 'Float', 'Qubit', 'QRegister', 'Tensor', 'String', 'Unit',
];

// Operators
const operators = [
  '->', '=>', '==', '!=', '<=', '>=', '<<', '>>', '+=', '-=', '*=', '/=',
  '&&', '||', '|>', '<|', '..', '...',
];

// Delimiters
const delimiters = [
  '{', '}', '(', ')', '[', ']', ',', ';', ':', '.', '::', '@',
];

// Register Naso language
export function registerNasoLanguage() {
  monaco.languages.register({ id: NASO_LANGUAGE_ID });

  // Tokenizer
  monaco.languages.setMonarchTokensProvider(NASO_LANGUAGE_ID, {
    defaultToken: '',
    tokenPostfix: '.naso',
    keywords: keywords,
    typeKeywords: typeKeywords,
    operators: operators,
    symbols: /[=><!~?:&|+\-*/^%]+/,
    escapes: /\\(?:[abfnrtv\\"']|x[0-9A-Fa-f]{1,4}|u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8})/,
    
    tokenizer: {
      root: [
        // Comments
        [/\/\/.*$/, 'comment'],
        [/\/\*/, 'comment', '@comment'],
        
        // Quantity markers [0], [1], [*], [N]
        [/\[0\]/, 'quantity.zero'],
        [/\[1\]/, 'quantity.one'],
        [/\[\*\]/, 'quantity.many'],
        [/\[[0-9]+\]/, 'quantity.bounded'],
        [/\[[a-zA-Z_][a-zA-Z0-9_]*\]/, 'quantity.symbolic'],
        
        // Keywords
        [/\b(fn|let|mut|inout|consume|reversible|return)\b/, 'keyword.control'],
        [/\b(struct|enum|type|val|const|import|mod)\b/, 'keyword.declaration'],
        [/\b(if|else|while|for|loop|break|continue|match|as|is)\b/, 'keyword.control'],
        [/\b(true|false)\b/, 'keyword.literal'],
        
        // Types
        [/\b(Int|Bool|Float|Qubit|QRegister|Tensor|String|Unit|Self)\b/, 'type'],
        
        // Built-in functions
        [/\b(qalloc|hadamard|cnot|measure|qfree|linear_alloc|linear_free|alloc_tensor)\b/, 'function.builtin'],
        
        // Identifiers (type names start with uppercase)
        [/[A-Z][a-zA-Z0-9_]*/, 'type.identifier'],
        [/[a-z_][a-zA-Z0-9_]*/, 'identifier'],
        
        // Numbers
        [/\d+\.\d+([eE][+-]?\d+)?/, 'number.float'],
        [/\d+[uU]/, 'number.unsigned'],
        [/\d+/, 'number.integer'],
        
        // Strings
        [/"/, 'string', '@string'],
        
        // Characters
        [/'/, 'string.char', '@char'],
        
        // Operators and delimiters
        [/[{}()\[\]]/, '@brackets'],
        [/[<>](?!@symbols)/, '@brackets'],
        [/@symbols/, 'operator'],
        
        // Special: -> => = + - * / etc.
        [/->|=>|==|!=|<=|>=|<<|>>|\+=|-=|\*=|\/=|&&|\|\||[=+\-*/%<>!&|^~]/, 'operator'],
        
        // Delimiters
        [/[,:;.@]/, 'delimiter'],
      ],
      
      comment: [
        [/[^*]+/, 'comment'],
        [/\*\//, 'comment', '@pop'],
        [/[*/]/, 'comment'],
      ],
      
      string: [
        [/[^\\"]+/, 'string'],
        [/@escapes/, 'string.escape'],
        [/\\./, 'string.escape.invalid'],
        [/"/, 'string', '@pop'],
      ],
      
      char: [
        [/[^\\']+/, 'string.char'],
        [/@escapes/, 'string.escape'],
        [/\\./, 'string.escape.invalid'],
        [/'/, 'string.char', '@pop'],
      ],
    },
  });

  // Language configuration
  monaco.languages.setLanguageConfiguration(NASO_LANGUAGE_ID, {
    comments: {
      lineComment: '//',
      blockComment: ['/*', '*/'],
    },
    brackets: [
      ['{', '}'],
      ['[', ']'],
      ['(', ')'],
      ['<', '>'],
    ],
    autoClosingPairs: [
      { open: '{', close: '}' },
      { open: '[', close: ']' },
      { open: '(', close: ')' },
      { open: '"', close: '"' },
      { open: "'", close: "'" },
      { open: '/*', close: '*/' },
    ],
    surroundingPairs: [
      { open: '{', close: '}' },
      { open: '[', close: ']' },
      { open: '(', close: ')' },
      { open: '"', close: '"' },
      { open: "'", close: "'" },
    ],
    folding: {
      markers: {
        start: /^\s*\/\/\s*#region\b/,
        end: /^\s*\/\/\s*#endregion\b/,
      },
    },
    onEnterRules: [
      {
        beforeText: /^\s*\w.*\{$/,
        action: { indentAction: monaco.languages.IndentAction.IndentOutdent },
      },
      {
        beforeText: /^\s*\w.*[^\{]*$/,
        action: { indentAction: monaco.languages.IndentAction.None },
      },
    ],
  });

  // Define theme colors for Naso-specific tokens
  monaco.editor.defineTheme('naso-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'quantity.zero', foreground: 'ff7b72', fontStyle: 'bold' },
      { token: 'quantity.one', foreground: 'a5d6ff', fontStyle: 'bold' },
      { token: 'quantity.many', foreground: '79c0ff', fontStyle: 'bold' },
      { token: 'quantity.bounded', foreground: 'd2a8ff' },
      { token: 'quantity.symbolic', foreground: 'ffa657' },
      { token: 'keyword.control', foreground: 'ff7b72' },
      { token: 'keyword.declaration', foreground: 'ff7b72' },
      { token: 'keyword.literal', foreground: '79c0ff' },
      { token: 'type', foreground: '79c0ff' },
      { token: 'type.identifier', foreground: 'd2a8ff' },
      { token: 'function.builtin', foreground: 'a5d6ff' },
      { token: 'identifier', foreground: 'e6edf3' },
      { token: 'number', foreground: '79c0ff' },
      { token: 'string', foreground: 'a5d6ff' },
      { token: 'operator', foreground: 'ffa657' },
      { token: 'delimiter', foreground: '8b949e' },
      { token: 'comment', foreground: '8b949e', fontStyle: 'italic' },
    ],
    colors: {
      'editor.background': '#1e1e1e',
      'editor.foreground': '#e6edf3',
      'editor.lineHighlightBackground': '#2d333b',
      'editor.selectionBackground': '#264f78',
      'editor.inactiveSelectionBackground': '#3a3d41',
      'editorCursor.foreground': '#58a6ff',
      'editorWhitespace.foreground': '#30363d',
      'editorIndentGuide.background': '#30363d',
      'editorIndentGuide.activeBackground': '#484f58',
    },
  });
}

// Export default program for playground
export const DEFAULT_PROGRAM = `// Naso Playground - Reversible Quantum Adder Example

// Full adder: XOR inputs into pre-allocated output qubits, then measure.
// Each [1] qubit is used exactly once to respect linear types.
fn main() -> Int {
    let [1] q1: Qubit = qalloc();
    let [1] q2: Qubit = qalloc();
    let [1] q3: Qubit = qalloc();
    let [1] sum: Qubit = qalloc();
    let [1] carry: Qubit = qalloc();

    // Sum = q1 ^ q2 ^ q3
    cnot(q1, sum);
    cnot(q2, sum);
    cnot(q3, sum);

    // Carry = q1 ^ q2 ^ q3 (same computation for demo)
    cnot(q1, carry);
    cnot(q2, carry);
    cnot(q3, carry);

    // Measure and free all qubits (each consumed exactly once)
    let s: Bool = measure(sum);
    let c: Bool = measure(carry);
    let _a: Bool = measure(q1);
    let _b: Bool = measure(q2);
    let _d: Bool = measure(q3);

    if s { 1 } else { 0 } + if c { 2 } else { 0 }
}
`;