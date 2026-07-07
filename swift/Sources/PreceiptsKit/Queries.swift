// Tree-sitter highlight queries — mechanical port of the Rust registry
// (core/src/highlight/queries.rs). Zig is dropped (no SPM grammar);
// elixir uses the grammar's bundled highlights.scm (Apache-2.0,
// elixir-lang/tree-sitter-elixir v0.3.4).
//
// Regenerate with scratchpad convert-queries.py rather than editing the
// query bodies by hand.

enum HighlightQueries {
    static let typescript = #"""
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(true) @constant.builtin
(false) @constant.builtin
(null) @constant.builtin
(undefined) @constant.builtin
(regex) @string.special

["const" "let" "var" "function" "class" "interface" "type" "enum" "namespace" "module" "declare" "implements" "extends" "public" "private" "protected" "readonly" "static" "abstract" "async" "await" "return" "if" "else" "for" "while" "do" "switch" "case" "default" "break" "continue" "try" "catch" "finally" "throw" "new" "delete" "typeof" "instanceof" "in" "of" "as" "is" "import" "export" "from" "default" "void"] @keyword

(type_identifier) @type
(predefined_type) @type.builtin

(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function.method)
(call_expression function: (identifier) @function)
(call_expression function: (member_expression property: (property_identifier) @function.method))
(arrow_function) @function

(property_identifier) @property
(shorthand_property_identifier) @property
(shorthand_property_identifier_pattern) @property

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["." "," ";" ":"] @punctuation.delimiter
"""#

    static let tsx = #"""
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(true) @constant.builtin
(false) @constant.builtin
(null) @constant.builtin
(undefined) @constant.builtin
(regex) @string.special

["const" "let" "var" "function" "class" "interface" "type" "enum" "namespace" "module" "declare" "implements" "extends" "public" "private" "protected" "readonly" "static" "abstract" "async" "await" "return" "if" "else" "for" "while" "do" "switch" "case" "default" "break" "continue" "try" "catch" "finally" "throw" "new" "delete" "typeof" "instanceof" "in" "of" "as" "is" "import" "export" "from" "default" "void"] @keyword

(type_identifier) @type
(predefined_type) @type.builtin

(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function.method)
(call_expression function: (identifier) @function)
(call_expression function: (member_expression property: (property_identifier) @function.method))
(arrow_function) @function

(property_identifier) @property
(shorthand_property_identifier) @property
(shorthand_property_identifier_pattern) @property

(jsx_element open_tag: (jsx_opening_element name: (identifier) @tag))
(jsx_element close_tag: (jsx_closing_element name: (identifier) @tag))
(jsx_self_closing_element name: (identifier) @tag)
(jsx_attribute (property_identifier) @attribute)

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["." "," ";" ":"] @punctuation.delimiter
"""#

    static let javascript = #"""
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(true) @constant.builtin
(false) @constant.builtin
(null) @constant.builtin
(undefined) @constant.builtin
(regex) @string.special

["const" "let" "var" "function" "class" "extends" "async" "await" "return" "if" "else" "for" "while" "do" "switch" "case" "default" "break" "continue" "try" "catch" "finally" "throw" "new" "delete" "typeof" "instanceof" "in" "of" "import" "export" "from" "default" "void"] @keyword

(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function.method)
(call_expression function: (identifier) @function)
(call_expression function: (member_expression property: (property_identifier) @function.method))
(arrow_function) @function

(property_identifier) @property
(shorthand_property_identifier) @property

(jsx_element open_tag: (jsx_opening_element name: (identifier) @tag))
(jsx_element close_tag: (jsx_closing_element name: (identifier) @tag))
(jsx_self_closing_element name: (identifier) @tag)
(jsx_attribute (property_identifier) @attribute)

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["." "," ";" ":"] @punctuation.delimiter
"""#

    static let rust = #"""
; Comments
; Regular comments (line_comment and block_comment capture the entire comment)
(line_comment) @comment
(block_comment) @comment
; Doc comment parts need explicit captures to prevent operator conflicts
; The "/" in "///" and "!" in "//!" would otherwise match operator patterns
(outer_doc_comment_marker) @comment
(inner_doc_comment_marker) @comment
(doc_comment) @comment

; Strings and literals
(string_literal) @string
(raw_string_literal) @string
(char_literal) @string
(integer_literal) @number
(float_literal) @number
(boolean_literal) @constant.builtin

; Types
(type_identifier) @type
(primitive_type) @type.builtin

; Functions
(function_item (identifier) @function)
(function_signature_item (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @function.method))
(call_expression function: (scoped_identifier name: (identifier) @function))
(generic_function function: (identifier) @function)
(generic_function function: (scoped_identifier name: (identifier) @function))

; Macros
(macro_invocation macro: (identifier) @function.macro "!" @function.macro)
(macro_definition "macro_rules!" @function.macro)

; Fields and properties
(field_identifier) @variable.member
(shorthand_field_identifier) @variable.member

; Labels and lifetimes
(lifetime (identifier) @label)

; Parameters
(parameter (identifier) @variable.parameter)

; Modules
(mod_item name: (identifier) @module)
(scoped_identifier path: (identifier) @module)

; Self, crate, and special
(self) @variable.builtin
(crate) @keyword
(super) @keyword
(mutable_specifier) @keyword

; Keywords
"as" @keyword
"async" @keyword
"await" @keyword
"break" @keyword
"const" @keyword
"continue" @keyword
"dyn" @keyword
"else" @keyword
"enum" @keyword
"extern" @keyword
"fn" @keyword
"for" @keyword
"if" @keyword
"impl" @keyword
"in" @keyword
"let" @keyword
"loop" @keyword
"match" @keyword
"mod" @keyword
"move" @keyword
"pub" @keyword
"ref" @keyword
"return" @keyword
"static" @keyword
"struct" @keyword
"trait" @keyword
"type" @keyword
"unsafe" @keyword
"use" @keyword
"where" @keyword
"while" @keyword

; Operators
; Note: "/" and "!" are not matched globally to avoid conflicts with doc comments
; They are highlighted via binary_expression and unary_expression patterns below
"*" @operator
"&" @operator
"=" @operator
"+" @operator
"-" @operator
"%" @operator
"<" @operator
">" @operator
"==" @operator
"!=" @operator
"<=" @operator
">=" @operator
"&&" @operator
"||" @operator
"+=" @operator
"-=" @operator
"*=" @operator
"/=" @operator
".." @operator
"..=" @operator
"=>" @operator
"->" @operator
"?" @operator

; Division and negation operators in specific contexts
(binary_expression "/" @operator)
(unary_expression "!" @operator)

; Punctuation
"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"::" @punctuation.delimiter
":" @punctuation.delimiter
"""#

    static let ruby = #"""
; Comments
(comment) @comment

; Strings and symbols
(string) @string
(bare_string) @string
(subshell) @string
(heredoc_body) @string
(heredoc_beginning) @string
(simple_symbol) @string.special
(delimited_symbol) @string.special
(hash_key_symbol) @string.special
(bare_symbol) @string.special
(regex) @string.special

; Literals
(integer) @number
(float) @number
(nil) @constant.builtin
(true) @constant.builtin
(false) @constant.builtin

; Constants
(constant) @type

; Variables
(instance_variable) @property
(class_variable) @property
(global_variable) @variable.builtin
(self) @variable.builtin
(super) @variable.builtin

; Parameters
(block_parameter (identifier) @variable.parameter)
(block_parameters (identifier) @variable.parameter)
(method_parameters (identifier) @variable.parameter)
(keyword_parameter name: (identifier) @variable.parameter)
(optional_parameter name: (identifier) @variable.parameter)
(splat_parameter (identifier) @variable.parameter)
(hash_splat_parameter (identifier) @variable.parameter)

; Functions and methods
(method name: (identifier) @function)
(method name: (constant) @function)
(singleton_method name: (identifier) @function)
(call method: (identifier) @function.method)
(call method: (constant) @function.method)

; Keywords
"alias" @keyword
"and" @keyword
"begin" @keyword
"break" @keyword
"case" @keyword
"class" @keyword
"def" @keyword
"do" @keyword
"else" @keyword
"elsif" @keyword
"end" @keyword
"ensure" @keyword
"for" @keyword
"if" @keyword
"in" @keyword
"module" @keyword
"next" @keyword
"or" @keyword
"rescue" @keyword
"retry" @keyword
"return" @keyword
"then" @keyword
"unless" @keyword
"until" @keyword
"when" @keyword
"while" @keyword
"yield" @keyword
"not" @keyword
"defined?" @keyword

; Operators
"=" @operator
"=>" @operator
"->" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"%" @operator
"**" @operator
"==" @operator
"!=" @operator
"<" @operator
">" @operator
"<=" @operator
">=" @operator
"<=>" @operator
"&&" @operator
"||" @operator
"!" @operator
"&" @operator
"|" @operator
"^" @operator
"~" @operator
"<<" @operator
">>" @operator
".." @operator
"..." @operator

; Punctuation
"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"," @punctuation.delimiter
";" @punctuation.delimiter
"." @punctuation.delimiter
":" @punctuation.delimiter
"::" @punctuation.delimiter
"""#

    static let json = #"""
(string) @string
(number) @number
(true) @constant.builtin
(false) @constant.builtin
(null) @constant.builtin
(pair key: (string) @property)

"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
":" @punctuation.delimiter
"," @punctuation.delimiter
"""#

    static let python = #"""
; Comments and strings
(comment) @comment
(string) @string
(escape_sequence) @string.special

; Literals
(integer) @number
(float) @number
(none) @constant.builtin
(true) @constant.builtin
(false) @constant.builtin

; Types and attributes
(type (identifier) @type)
(attribute attribute: (identifier) @property)

; Functions
(function_definition name: (identifier) @function)
(call function: (identifier) @function)
(call function: (attribute attribute: (identifier) @function.method))
(decorator) @function
(decorator (identifier) @function)

; Keywords
"as" @keyword
"assert" @keyword
"async" @keyword
"await" @keyword
"break" @keyword
"class" @keyword
"continue" @keyword
"def" @keyword
"del" @keyword
"elif" @keyword
"else" @keyword
"except" @keyword
"finally" @keyword
"for" @keyword
"from" @keyword
"global" @keyword
"if" @keyword
"import" @keyword
"lambda" @keyword
"nonlocal" @keyword
"pass" @keyword
"raise" @keyword
"return" @keyword
"try" @keyword
"while" @keyword
"with" @keyword
"yield" @keyword
"match" @keyword
"case" @keyword
"and" @operator
"or" @operator
"not" @operator
"in" @operator
"is" @operator
"""#

    static let go = #"""
; Comments and strings
(comment) @comment
(interpreted_string_literal) @string
(raw_string_literal) @string
(rune_literal) @string

; Literals
(int_literal) @number
(float_literal) @number
(true) @constant.builtin
(false) @constant.builtin
(nil) @constant.builtin

; Types
(type_identifier) @type
(type_spec name: (type_identifier) @type)

; Functions
(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @function.method)
(call_expression function: (identifier) @function)
(call_expression function: (selector_expression field: (field_identifier) @function.method))

; Fields
(field_identifier) @property

; Package
(package_identifier) @module

; Keywords
"break" @keyword
"case" @keyword
"chan" @keyword
"const" @keyword
"continue" @keyword
"default" @keyword
"defer" @keyword
"else" @keyword
"fallthrough" @keyword
"for" @keyword
"func" @keyword
"go" @keyword
"goto" @keyword
"if" @keyword
"import" @keyword
"interface" @keyword
"map" @keyword
"package" @keyword
"range" @keyword
"return" @keyword
"select" @keyword
"struct" @keyword
"switch" @keyword
"type" @keyword
"var" @keyword

; Operators
"=" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"%" @operator
"!" @operator
"<" @operator
">" @operator
"&" @operator
"|" @operator
"^" @operator
":=" @operator
"==" @operator
"!=" @operator
"<=" @operator
">=" @operator
"&&" @operator
"||" @operator
"++" @operator
"--" @operator
"+=" @operator
"-=" @operator
"<-" @operator

; Punctuation
"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"." @punctuation.delimiter
"," @punctuation.delimiter
";" @punctuation.delimiter
":" @punctuation.delimiter
"""#

    static let css = #"""
(comment) @comment
(string_value) @string
(integer_value) @number
(float_value) @number
(color_value) @constant
(property_name) @property
(tag_name) @tag
(class_name) @type
(id_name) @constant
(at_keyword) @keyword
"""#

    static let html = #"""
(comment) @comment
(quoted_attribute_value) @string
(tag_name) @tag
(attribute_name) @attribute
"""#

    static let toml = #"""
(comment) @comment
(string) @string
(integer) @number
(float) @number
(boolean) @constant.builtin
(bare_key) @property
(dotted_key) @property
"""#

    static let bash = #"""
(comment) @comment
(string) @string
(raw_string) @string
(number) @number
(command_name) @function
(variable_name) @variable
"""#

    static let markdown = #"""
(atx_heading) @keyword
(setext_heading) @keyword
(thematic_break) @punctuation.delimiter
(fenced_code_block) @string
(indented_code_block) @string
(block_quote) @comment
(list_marker_plus) @punctuation
(list_marker_minus) @punctuation
(list_marker_star) @punctuation
(list_marker_dot) @punctuation
(list_marker_parenthesis) @punctuation
(link_destination) @string
(link_title) @string
"""#

    static let csharp = #"""
; Comments
(comment) @comment

; Strings and literals
(string_literal) @string
(verbatim_string_literal) @string
(interpolated_string_expression) @string
(character_literal) @string
(integer_literal) @number
(real_literal) @number
(boolean_literal) @constant.builtin
(null_literal) @constant.builtin

; Types - C# doesn't have type_identifier, types are represented by identifier in context
; or predefined_type for built-in types
(predefined_type) @type.builtin

; Namespaces and usings
(namespace_declaration name: (qualified_name) @module)
(namespace_declaration name: (identifier) @module)
(using_directive (identifier) @module)
(using_directive (qualified_name) @module)

; Classes, structs, interfaces, enums
(class_declaration name: (identifier) @type)
(struct_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(record_declaration name: (identifier) @type)

; Methods and functions
(method_declaration name: (identifier) @function)
(local_function_statement name: (identifier) @function)
(constructor_declaration name: (identifier) @function)
(destructor_declaration name: (identifier) @function)
(invocation_expression function: (identifier) @function)
(invocation_expression function: (member_access_expression name: (identifier) @function.method))

; Properties and fields
(property_declaration name: (identifier) @property)
(field_declaration (variable_declaration (variable_declarator (identifier) @variable.member)))

; Parameters
(parameter name: (identifier) @variable.parameter)

; Attributes
(attribute) @attribute
(attribute_list) @attribute

; Keywords
"abstract" @keyword
"as" @keyword
"async" @keyword
"await" @keyword
"base" @keyword
"break" @keyword
"case" @keyword
"catch" @keyword
"checked" @keyword
"class" @keyword
"const" @keyword
"continue" @keyword
"default" @keyword
"delegate" @keyword
"do" @keyword
"else" @keyword
"enum" @keyword
"event" @keyword
"explicit" @keyword
"extern" @keyword
"finally" @keyword
"fixed" @keyword
"for" @keyword
"foreach" @keyword
"goto" @keyword
"if" @keyword
"implicit" @keyword
"in" @keyword
"interface" @keyword
"internal" @keyword
"is" @keyword
"lock" @keyword
"namespace" @keyword
"new" @keyword
"operator" @keyword
"out" @keyword
"override" @keyword
"params" @keyword
"private" @keyword
"protected" @keyword
"public" @keyword
"readonly" @keyword
"record" @keyword
"ref" @keyword
"return" @keyword
"sealed" @keyword
"sizeof" @keyword
"stackalloc" @keyword
"static" @keyword
"struct" @keyword
"switch" @keyword
"this" @keyword
"throw" @keyword
"try" @keyword
"typeof" @keyword
"unchecked" @keyword
"unsafe" @keyword
"using" @keyword
"var" @keyword
"virtual" @keyword
"volatile" @keyword
"when" @keyword
"where" @keyword
"while" @keyword
"yield" @keyword
"get" @keyword
"set" @keyword
"init" @keyword
"add" @keyword
"remove" @keyword
"partial" @keyword
"global" @keyword
"required" @keyword
"file" @keyword
"scoped" @keyword

; Operators
"=" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"%" @operator
"!" @operator
"<" @operator
">" @operator
"&" @operator
"|" @operator
"^" @operator
"~" @operator
"?" @operator
"==" @operator
"!=" @operator
"<=" @operator
">=" @operator
"&&" @operator
"||" @operator
"+=" @operator
"-=" @operator
"*=" @operator
"/=" @operator
"??" @operator
"??=" @operator
"=>" @operator
"->" @operator
"++" @operator
"--" @operator

; Punctuation
"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"." @punctuation.delimiter
"," @punctuation.delimiter
";" @punctuation.delimiter
":" @punctuation.delimiter
"""#

    static let elixir = #"""
; Punctuation

[
 "%"
] @punctuation

[
 ","
 ";"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "<<"
  ">>"
] @punctuation.bracket

; Literals

[
  (boolean)
  (nil)
] @constant

[
  (integer)
  (float)
] @number

(char) @constant

; Identifiers

; * regular
(identifier) @variable

; * unused
(
  (identifier) @comment.unused
  (#match? @comment.unused "^_")
)

; * special
(
  (identifier) @constant.builtin
  (#any-of? @constant.builtin "__MODULE__" "__DIR__" "__ENV__" "__CALLER__" "__STACKTRACE__")
)

; Comment

(comment) @comment

; Quoted content

(interpolation "#{" @punctuation.special "}" @punctuation.special) @embedded

(escape_sequence) @string.escape

[
  (string)
  (charlist)
] @string

[
  (atom)
  (quoted_atom)
  (keyword)
  (quoted_keyword)
] @string.special.symbol

; Note that we explicitly target sigil quoted start/end, so they are not overridden by delimiters

(sigil
  (sigil_name) @__name__
  quoted_start: _ @string.special
  quoted_end: _ @string.special) @string.special

(sigil
  (sigil_name) @__name__
  quoted_start: _ @string
  quoted_end: _ @string
  (#match? @__name__ "^[sS]$")) @string

(sigil
  (sigil_name) @__name__
  quoted_start: _ @string.regex
  quoted_end: _ @string.regex
  (#match? @__name__ "^[rR]$")) @string.regex

; Calls

; * local function call
(call
  target: (identifier) @function)

; * remote function call
(call
  target: (dot
    right: (identifier) @function))

; * field without parentheses or block
(call
  target: (dot
    right: (identifier) @property)
  .)

; * remote call without parentheses or block (overrides above)
(call
  target: (dot
    left: [
      (alias)
      (atom)
    ]
    right: (identifier) @function)
  .)

; * definition keyword
(call
  target: (identifier) @keyword
  (#any-of? @keyword "def" "defdelegate" "defexception" "defguard" "defguardp" "defimpl" "defmacro" "defmacrop" "defmodule" "defn" "defnp" "defoverridable" "defp" "defprotocol" "defstruct"))

; * kernel or special forms keyword
(call
  target: (identifier) @keyword
  (#any-of? @keyword "alias" "case" "cond" "for" "if" "import" "quote" "raise" "receive" "require" "reraise" "super" "throw" "try" "unless" "unquote" "unquote_splicing" "use" "with"))

; * just identifier in function definition
(call
  target: (identifier) @keyword
  (arguments
    [
      (identifier) @function
      (binary_operator
        left: (identifier) @function
        operator: "when")
    ])
  (#any-of? @keyword "def" "defdelegate" "defguard" "defguardp" "defmacro" "defmacrop" "defn" "defnp" "defp"))

; * pipe into identifier (function call)
(binary_operator
  operator: "|>"
  right: (identifier) @function)

; * pipe into identifier (definition)
(call
  target: (identifier) @keyword
  (arguments
    (binary_operator
      operator: "|>"
      right: (identifier) @variable))
  (#any-of? @keyword "def" "defdelegate" "defguard" "defguardp" "defmacro" "defmacrop" "defn" "defnp" "defp"))

; * pipe into field without parentheses (function call)
(binary_operator
  operator: "|>"
  right: (call
    target: (dot
      right: (identifier) @function)))

; Operators

; * capture operand
(unary_operator
  operator: "&"
  operand: (integer) @operator)

(operator_identifier) @operator

(unary_operator
  operator: _ @operator)

(binary_operator
  operator: _ @operator)

(dot
  operator: _ @operator)

(stab_clause
  operator: _ @operator)

; * module attribute
(unary_operator
  operator: "@" @attribute
  operand: [
    (identifier) @attribute
    (call
      target: (identifier) @attribute)
    (boolean) @attribute
    (nil) @attribute
  ])

; * doc string
(unary_operator
  operator: "@" @comment.doc
  operand: (call
    target: (identifier) @comment.doc.__attribute__
    (arguments
      [
        (string) @comment.doc
        (charlist) @comment.doc
        (sigil
          quoted_start: _ @comment.doc
          quoted_end: _ @comment.doc) @comment.doc
        (boolean) @comment.doc
      ]))
  (#any-of? @comment.doc.__attribute__ "moduledoc" "typedoc" "doc"))

; Module

(alias) @module

(call
  target: (dot
    left: (atom) @module))

; Reserved keywords

["when" "and" "or" "not" "in" "not in" "fn" "do" "end" "catch" "rescue" "after" "else"] @keyword
"""#
}
