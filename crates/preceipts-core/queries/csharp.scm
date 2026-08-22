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
