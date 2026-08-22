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
