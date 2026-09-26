# gridcalc

A spreadsheet engine over CSV: `=formulas` with + - * / parens, unary
minus, cell refs (incl. AA-style columns), and SUM/AVG/MIN/MAX/COUNT over
ranges. Evaluation memoizes and detects reference cycles BY CELL NAME —
`=A1` in A1 is an error message, not a stack overflow. Text is 0 for
math; COUNT counts nonempty cells; semantics are in the tests, not in
anyone's imagination.

    gridcalc eval sheet.csv C7
    gridcalc recalc sheet.csv --out values.csv
