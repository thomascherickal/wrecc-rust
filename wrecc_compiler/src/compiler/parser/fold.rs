use crate::compiler::ast::decl::DeclType;
use crate::compiler::ast::expr::*;
use crate::compiler::codegen::register::*;
use crate::compiler::common::{environment::*, error::*, token::*, types::*};
use crate::compiler::typechecker::*;

impl Expr {
    pub fn new_literal(value: i64, primitive_type: Types) -> Self {
        Expr {
            type_decl: Some(NEWTypes::Primitive(primitive_type)),
            kind: ExprKind::Literal(value),
            value_kind: ValueKind::Rvalue,
        }
    }
    pub fn get_literal_constant(
        &mut self,
        typechecker: &mut TypeChecker,
        token: &Token,
        msg: &'static str,
    ) -> Result<i64, Error> {
        self.integer_const_fold(typechecker)?;

        if let ExprKind::Literal(n) = self.kind {
            Ok(n)
        } else {
            Err(Error::new(token, ErrorKind::NotIntegerConstant(msg)))
        }
    }
    pub fn preprocessor_constant(&mut self, pp: &impl Location) -> Result<i64, Error> {
        let mut typechecker = TypeChecker::new();
        self.integer_const_fold(&mut typechecker)?;

        if let ExprKind::Literal(n) = self.kind {
            Ok(n)
        } else {
            Err(Error::new(
                pp,
                ErrorKind::Regular("Invalid preprocessor constant expression"),
            ))
        }
    }
    // https://en.cppreference.com/w/c/language/constant_expression
    pub fn integer_const_fold(&mut self, typechecker: &mut TypeChecker) -> Result<(), Error> {
        let folded: Option<Expr> = match &mut self.kind {
            ExprKind::Literal(_) => None,
            ExprKind::Ident(name) => {
                // if variable is known at compile time then foldable
                // is only enum-constant
                if let Ok(Symbols::Variable(SymbolInfo {
                    reg: Some(Register::Literal(n, _)),
                    type_decl,
                    ..
                })) = typechecker.env.get_symbol(name).map(|s| s.borrow().clone())
                {
                    Some(Expr::new_literal(
                        n,
                        type_decl.get_primitive().unwrap().clone(),
                    ))
                } else {
                    None
                }
            }
            ExprKind::Binary { left, token, right } => {
                Self::binary_fold(typechecker, left, token.clone(), right)?
            }
            ExprKind::Unary { token, right, .. } => {
                Self::unary_fold(typechecker, token.clone(), right)?
            }
            ExprKind::Logical { left, token, right } => {
                Self::logical_fold(typechecker, token.clone(), left, right)?
            }
            ExprKind::Comparison { left, token, right } => {
                Self::comp_fold(typechecker, token.clone(), left, right)?
            }
            ExprKind::Cast { decl_type, token, expr, .. } => {
                Self::const_cast(typechecker, token.clone(), decl_type, expr)?
            }
            ExprKind::Grouping { expr } => {
                expr.integer_const_fold(typechecker)?;
                Some(expr.as_ref().clone())
            }
            ExprKind::Ternary { cond, true_expr, false_expr, token } => {
                cond.integer_const_fold(typechecker)?;
                true_expr.integer_const_fold(typechecker)?;
                false_expr.integer_const_fold(typechecker)?;

                if let (ExprKind::Literal(_), ExprKind::Literal(_)) =
                    (&true_expr.kind, &false_expr.kind)
                {
                    let true_type = true_expr.type_decl.clone().unwrap();
                    let false_type = false_expr.type_decl.clone().unwrap();

                    if !true_type.type_compatible(&false_type, &false_expr.kind) {
                        return Err(Error::new(
                            token,
                            ErrorKind::TypeMismatch(true_type, false_type),
                        ));
                    }
                }

                match &cond.kind {
                    ExprKind::Literal(0) => Some(false_expr.as_ref().clone()),
                    ExprKind::Literal(_) => Some(true_expr.as_ref().clone()),
                    _ => None,
                }
            }
            ExprKind::SizeofType { decl_type, .. } => {
                let size = typechecker.parse_type(decl_type)?.size();
                Some(Expr::new_literal(size as i64, integer_type(size as i64)))
            }

            ExprKind::ScaleUp { expr, .. } | ExprKind::ScaleDown { expr, .. } => {
                expr.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::Assign { l_expr, r_expr, .. } => {
                l_expr.integer_const_fold(typechecker)?;
                r_expr.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::CompoundAssign { l_expr, r_expr, .. } => {
                l_expr.integer_const_fold(typechecker)?;
                r_expr.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::Comma { left, right } => {
                left.integer_const_fold(typechecker)?;
                right.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::MemberAccess { expr, .. } => {
                expr.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::PostUnary { left, .. } => {
                left.integer_const_fold(typechecker)?;
                None
            }
            ExprKind::Call { .. }
            | ExprKind::String(..)
            | ExprKind::SizeofExpr { .. }
            | ExprKind::Nop { .. } => None,
        };

        if let Some(folded_expr) = folded {
            *self = folded_expr;
        };
        Ok(())
    }
    fn binary_fold(
        typechecker: &mut TypeChecker,
        left: &mut Box<Expr>,
        token: Token,
        right: &mut Box<Expr>,
    ) -> Result<Option<Expr>, Error> {
        left.integer_const_fold(typechecker)?;
        right.integer_const_fold(typechecker)?;

        if let (ExprKind::Literal(mut left_n), ExprKind::Literal(mut right_n)) =
            (&mut left.kind, &mut right.kind)
        {
            let (left_type, right_type) = (
                left.type_decl.clone().unwrap(),
                right.type_decl.clone().unwrap(),
            );
            if !crate::compiler::typechecker::is_valid_bin(
                &token,
                &left_type,
                &right_type,
                &right.kind,
            ) {
                return Err(Error::new(
                    &token,
                    ErrorKind::InvalidBinary(token.token.clone(), left_type, right_type),
                ));
            }

            if let Some((literal, amount)) =
                maybe_scale(&left_type, &right_type, &mut left_n, &mut right_n)
            {
                *literal *= amount as i64;
            }

            Ok(Some(match token.token {
                TokenType::Plus => Self::literal_type(
                    token,
                    left_type,
                    right_type,
                    i64::overflowing_add(left_n, right_n),
                )?,
                TokenType::Minus => Self::literal_type(
                    token,
                    left_type,
                    right_type,
                    i64::overflowing_sub(left_n, right_n),
                )?,
                TokenType::Star => Self::literal_type(
                    token,
                    left_type,
                    right_type,
                    i64::overflowing_mul(left_n, right_n),
                )?,
                TokenType::Slash | TokenType::Mod => {
                    Self::div_fold(token, left_type, right_type, left_n, right_n)?
                }
                TokenType::GreaterGreater | TokenType::LessLess => {
                    Self::shift_fold(token, left_type, left_n, right_n)?
                }

                TokenType::Pipe => {
                    Self::literal_type(token, left_type, right_type, (left_n | right_n, false))?
                }
                TokenType::Xor => {
                    Self::literal_type(token, left_type, right_type, (left_n ^ right_n, false))?
                }
                TokenType::Amp => {
                    Self::literal_type(token, left_type, right_type, (left_n & right_n, false))?
                }

                _ => unreachable!("not binary token"),
            }))
        } else {
            Ok(None)
        }
    }
    fn shift_fold(token: Token, left_type: NEWTypes, left: i64, right: i64) -> Result<Expr, Error> {
        // result type is only dependant on left operand
        let left_type = if left_type.size() < Types::Int.size() {
            NEWTypes::Primitive(Types::Int)
        } else {
            left_type
        };
        if right < 0 {
            return Err(Error::new(&token, ErrorKind::NegativeShift));
        }

        let (value, overflow) = match token.token {
            TokenType::GreaterGreater => i64::overflowing_shr(left, right as u32),
            TokenType::LessLess => i64::overflowing_shl(left, right as u32),
            _ => unreachable!("not shift operation"),
        };

        if overflow || Self::type_overflow(value, &left_type) {
            Err(Error::new(&token, ErrorKind::IntegerOverflow(left_type)))
        } else {
            Ok(Expr {
                kind: ExprKind::Literal(value),
                type_decl: Some(left_type),
                value_kind: ValueKind::Rvalue,
            })
        }
    }
    fn div_fold(
        token: Token,
        left_type: NEWTypes,
        right_type: NEWTypes,
        left: i64,
        right: i64,
    ) -> Result<Expr, Error> {
        if right == 0 {
            return Err(Error::new(&token, ErrorKind::DivideByZero));
        }

        let operation = match token.token {
            TokenType::Slash => i64::overflowing_div(left, right),
            TokenType::Mod => i64::overflowing_rem(left, right),
            _ => unreachable!("not shift operation"),
        };
        Self::literal_type(token, left_type, right_type, operation)
    }
    fn literal_type(
        token: Token,
        left_type: NEWTypes,
        right_type: NEWTypes,
        (value, overflow): (i64, bool),
    ) -> Result<Expr, Error> {
        let result_type = if left_type.size() > right_type.size() {
            left_type
        } else {
            right_type
        };
        let result_type = if result_type.size() < Types::Int.size() {
            NEWTypes::Primitive(Types::Int)
        } else {
            result_type
        };

        // calculation can overflow or type from literal can overflow
        if overflow || Self::type_overflow(value, &result_type) {
            Err(Error::new(&token, ErrorKind::IntegerOverflow(result_type)))
        } else {
            Ok(Expr {
                kind: ExprKind::Literal(value),
                type_decl: Some(result_type),
                value_kind: ValueKind::Rvalue,
            })
        }
    }
    fn type_overflow(value: i64, type_decl: &NEWTypes) -> bool {
        (value > type_decl.max()) || ((value) < type_decl.min())
    }

    fn unary_fold(
        typechecker: &mut TypeChecker,
        token: Token,
        right: &mut Box<Expr>,
    ) -> Result<Option<Expr>, Error> {
        right.integer_const_fold(typechecker)?;

        Ok(match (&right.kind, &token.token) {
            (ExprKind::Literal(n), TokenType::Bang) => {
                Some(Expr::new_literal(if *n == 0 { 1 } else { 0 }, Types::Int))
            }
            (ExprKind::Literal(n), TokenType::Minus | TokenType::Plus | TokenType::Tilde) => {
                let right_type = right.type_decl.clone().unwrap();

                if !right_type.is_integer() {
                    return Err(Error::new(
                        &token,
                        ErrorKind::InvalidUnary(token.token.clone(), right_type, "integer"),
                    ));
                }

                Some(Self::literal_type(
                    token.clone(),
                    right_type.clone(),
                    right_type,
                    if token.token == TokenType::Plus {
                        (*n, false)
                    } else if token.token == TokenType::Tilde {
                        (!n, false)
                    } else {
                        n.overflowing_neg()
                    },
                )?)
            }
            (..) => None,
        })
    }
    fn logical_fold(
        typechecker: &mut TypeChecker,
        token: Token,
        left: &mut Box<Expr>,
        right: &mut Box<Expr>,
    ) -> Result<Option<Expr>, Error> {
        left.integer_const_fold(typechecker)?;
        right.integer_const_fold(typechecker)?;

        Ok(match token.token {
            TokenType::AmpAmp => match (&left.kind, &right.kind) {
                (ExprKind::Literal(0), _) => Some(Expr::new_literal(0, Types::Int)),
                (ExprKind::Literal(left_n), ExprKind::Literal(right_n))
                    if *left_n != 0 && *right_n != 0 =>
                {
                    Some(Expr::new_literal(1, Types::Int))
                }
                (ExprKind::Literal(_), ExprKind::Literal(_)) => {
                    Some(Expr::new_literal(0, Types::Int))
                }
                _ => None,
            },

            TokenType::PipePipe => match (&left.kind, &right.kind) {
                (ExprKind::Literal(1), _) => Some(Expr::new_literal(1, Types::Int)),
                (ExprKind::Literal(left), ExprKind::Literal(right))
                    if *left == 0 && *right == 0 =>
                {
                    Some(Expr::new_literal(0, Types::Int))
                }
                (ExprKind::Literal(_), ExprKind::Literal(_)) => {
                    Some(Expr::new_literal(1, Types::Int))
                }
                _ => None,
            },
            _ => unreachable!("not logical token"),
        })
    }
    fn comp_fold(
        typechecker: &mut TypeChecker,
        token: Token,
        left: &mut Box<Expr>,
        right: &mut Box<Expr>,
    ) -> Result<Option<Expr>, Error> {
        left.integer_const_fold(typechecker)?;
        right.integer_const_fold(typechecker)?;

        if let (ExprKind::Literal(left_n), ExprKind::Literal(right_n)) = (&left.kind, &right.kind) {
            let (left_type, right_type) = (
                left.type_decl.clone().unwrap(),
                right.type_decl.clone().unwrap(),
            );

            if !left_type.type_compatible(&right_type, &right.kind)
                || left_type.is_void()
                || right_type.is_void()
            {
                return Err(Error::new(
                    &token,
                    ErrorKind::InvalidComp(token.token.clone(), left_type, right_type),
                ));
            }

            Ok(Some(match token.token {
                TokenType::BangEqual => Expr::new_literal((left_n != right_n).into(), Types::Int),
                TokenType::EqualEqual => Expr::new_literal((left_n == right_n).into(), Types::Int),

                TokenType::Greater => Expr::new_literal((left_n > right_n).into(), Types::Int),
                TokenType::GreaterEqual => {
                    Expr::new_literal((left_n >= right_n).into(), Types::Int)
                }
                TokenType::Less => Expr::new_literal((left_n < right_n).into(), Types::Int),
                TokenType::LessEqual => Expr::new_literal((left_n <= right_n).into(), Types::Int),
                _ => unreachable!("not valid comparison token"),
            }))
        } else {
            Ok(None)
        }
    }
    fn const_cast(
        typechecker: &mut TypeChecker,
        token: Token,
        decl_type: &mut DeclType,
        expr: &mut Box<Expr>,
    ) -> Result<Option<Expr>, Error> {
        expr.integer_const_fold(typechecker)?;

        if let ExprKind::Literal(right) = expr.kind {
            let new_type = typechecker.parse_type(decl_type)?;

            let (n, new_type) =
                Self::valid_cast(token, expr.type_decl.clone().unwrap(), new_type, right)?;
            Ok(Some(Expr {
                kind: ExprKind::Literal(n),
                type_decl: Some(new_type),
                value_kind: ValueKind::Rvalue,
            }))
        } else {
            Ok(None)
        }
    }
    fn valid_cast(
        token: Token,
        old_type: NEWTypes,
        new_type: NEWTypes,
        right_fold: i64,
    ) -> Result<(i64, NEWTypes), Error> {
        let result = match (&old_type, &new_type) {
            (old, new) if !old.is_scalar() || !new.is_scalar() => None,
            (_, NEWTypes::Primitive(Types::Char)) => Some(right_fold as i8 as i64),
            (_, NEWTypes::Primitive(Types::Int) | NEWTypes::Enum(..)) => {
                Some(right_fold as i32 as i64)
            }
            (_, NEWTypes::Primitive(Types::Long) | NEWTypes::Pointer(_)) => Some(right_fold),
            _ => None,
        };

        if let Some(result) = result {
            Ok((result, new_type))
        } else {
            Err(Error::new(
                &token,
                ErrorKind::InvalidConstCast(old_type, new_type),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::common::types::tests::setup_type;
    use crate::compiler::parser::tests::setup;

    fn assert_fold(input: &str, expected: &str) -> Option<NEWTypes> {
        let mut actual = setup(input).expression().unwrap();
        actual.integer_const_fold(&mut TypeChecker::new()).unwrap();

        let mut expected = setup(expected).expression().unwrap();
        expected
            .integer_const_fold(&mut TypeChecker::new())
            .unwrap();

        assert_eq!(actual, expected);

        return actual.type_decl;
    }
    fn assert_fold_type(input: &str, expected: &str, expected_type: &str) {
        let Some(actual_type) = assert_fold(input, expected) else {
            unreachable!("can only assert type of foldable expression");
        };
        assert_eq!(actual_type, setup_type(expected_type));
    }
    macro_rules! assert_fold_error {
        ($input:expr,$expected_err:pat) => {
            let actual_fold = setup($input)
                .expression()
                .unwrap()
                .integer_const_fold(&mut TypeChecker::new())
                .unwrap_err();

            assert!(
                matches!(actual_fold.kind, $expected_err),
                "expected: {}, found: {:?}",
                stringify!($expected_err),
                actual_fold
            );
        };
    }

    #[test]
    fn bit_fold() {
        assert_fold("-8 ^ -8", "0");
        assert_fold("1 ^ -8", "-7");

        assert_fold("-8 & -8", "-8");
        assert_fold("-8 & 1", "0");

        assert_fold("-8 | -8", "-8");
        assert_fold("1 | 1", "1");

        assert_fold_type("~0", "-1", "int");
    }
    #[test]
    fn advanced_bit_fold() {
        assert_fold("~4 + !'c'", "-5");
        assert_fold("~4 ^ -'c'", "102");
    }
    #[test]
    fn bit_vs_logical() {
        assert_fold("8 & 4", "0");
        assert_fold("8 && 4", "1");

        assert_fold("0 & 3", "0");
        assert_fold("1 && -0", "0");

        assert_fold("8 | 4", "12");
        assert_fold("8 || 4", "1");

        assert_fold("0 | 3", "3");
        assert_fold("1 || -0", "1");
    }

    #[test]
    fn comp_fold() {
        assert_fold("1 == 0", "0");
        assert_fold("-2 == -2", "1");

        assert_fold("3 != -2", "1");
        assert_fold("3 != 3", "0");

        assert_fold("3 > -2", "1");
        assert_fold("3 > 3", "0");

        assert_fold("3 >= -2", "1");
        assert_fold("3 >= 3", "1");

        assert_fold("'c' <= -2", "0");
        assert_fold("3 <= -2", "0");

        assert_fold("(long*)4 == (long*)1", "0");
        assert_fold("(long*)4 > (long*)1", "1");
        assert_fold_error!("(int*)4 <= 1", ErrorKind::InvalidComp(..));
        assert_fold_error!("(long*)4 > (char*)1", ErrorKind::InvalidComp(..));
    }

    #[test]
    fn shift_fold() {
        assert_fold("'1' << 5", "1568");
        assert_fold("'1' >> 2", "12");

        assert_fold_type("1 << (long)12", "4096", "int");
        assert_fold_type("(long)1 << (char)12", "(long)4096", "long");
        assert_fold_type("'1' << 12", "200704", "int");

        assert_fold_type("(long)-5 >> 42", "(long)-1", "long");
        assert_fold_type("(long)-5 << 42", "-21990232555520", "long");
    }
    #[test]
    fn shift_fold_error() {
        assert_fold_error!(
            "-16 << 33",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );
        assert_fold_error!(
            "(long)-5 >> 64",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Long))
        );

        // negative shift count is UB
        assert_fold_error!("16 << -2", ErrorKind::NegativeShift);
        assert_fold_error!("4 >> -3", ErrorKind::NegativeShift);

        assert_fold_error!("-16 << -2", ErrorKind::NegativeShift);
        assert_fold_error!("-5 >> -1", ErrorKind::NegativeShift);

        assert_fold_error!(
            "2147483647 << 2",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );
    }

    #[test]
    fn div_fold() {
        assert_fold("5 / 2", "2");
        assert_fold("32 % 5", "2");

        assert_fold("-5 / 2", "-2");
        assert_fold("-32 % 5", "-2");

        assert_fold("5 / -2", "-2");
        assert_fold("32 % -5", "2");

        assert_fold("-5 / -2", "2");
        assert_fold("-32 % -5", "-2");

        assert_fold("(34 / 3) * 3 + 34 % 3", "34");
    }
    #[test]
    fn div_fold_error() {
        assert_fold_error!("3 / 0", ErrorKind::DivideByZero);
        assert_fold_error!("-5 % 0", ErrorKind::DivideByZero);
    }

    #[test]
    fn char_fold() {
        assert_fold_type("'1' + '1'", "98", "int");
        assert_fold("-'1'", "-49");
        assert_fold("!-'1'", "0");
    }
    #[test]
    fn ptr_fold() {
        assert_fold_type("(long *)1 + 3", "(long*)25", "long*");
        assert_fold_type("2 + (char *)1 + 3", "(char*)6", "char*");

        assert_fold_error!(
            "6 / (int *)1",
            ErrorKind::InvalidBinary(_, NEWTypes::Primitive(Types::Int), NEWTypes::Pointer(_))
        );

        assert_fold_error!(
            "-(long *)1",
            ErrorKind::InvalidUnary(_, NEWTypes::Pointer(_), "integer")
        );
        assert_fold_error!(
            "~(char *)1",
            ErrorKind::InvalidUnary(_, NEWTypes::Pointer(_), "integer")
        );
    }
    #[test]
    fn ternary_fold() {
        assert_fold("1 == 2 ? 4 : 9", "9");
        assert_fold("(char*)1 ? 4 : 9", "4");

        assert_fold_type("1 - 2 ? (void*)4 : (long*)9 - 3", "(void*)4", "void*");

        assert_fold_error!(
            "1 == 2 ? 4 : (long*)9",
            ErrorKind::TypeMismatch(NEWTypes::Primitive(Types::Int), NEWTypes::Pointer(_))
        );

        assert_fold_error!(
            "1 == 2 ? (int*)4 : (long*)9",
            ErrorKind::TypeMismatch(NEWTypes::Pointer(_), NEWTypes::Pointer(_),)
        );
    }
    #[test]
    fn type_conversions() {
        assert_fold_type("4294967296 + 1", "4294967297", "long");
        assert_fold_type("2147483648 - 10", "(long)2147483638", "long");
        assert_fold_type("'1' * 2147483648", "105226698752", "long");

        assert_fold_type("'a'", "'a'", "char");
        assert_fold_type("-'a'", "-'a'", "int");
        assert_fold_type("+'a'", "(int)'a'", "int");

        assert_fold_type("2147483648", "2147483648", "long");

        assert_fold_type("-2147483649", "-2147483649", "long");
        assert_fold_type("(int)-2147483648", "(int)-2147483648", "int");
    }

    #[test]
    fn overflow_fold() {
        assert_fold_error!(
            "2147483647 + 1",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );
        assert_fold_error!(
            "9223372036854775807 * 2",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Long))
        );

        assert_fold_error!(
            "(int)-2147483648 - 1",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );
        assert_fold_error!(
            "(int)-2147483648 * -1",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );
        assert_fold_error!(
            "-((int)-2147483648)",
            ErrorKind::IntegerOverflow(NEWTypes::Primitive(Types::Int))
        );

        assert_fold_type("(char)127 + 2", "129", "int");
        assert_fold_type("2147483648 + 1", "2147483649", "long");
    }

    #[test]
    fn sub_fold_expressions() {
        assert_fold("1 + 2, 4 * 2", "3,8");
    }

    #[test]
    fn const_cast() {
        assert_fold_type("(long)'1' + '1'", "(long)98", "long");
        assert_fold_type("(char)2147483648", "(char)0", "char");
        assert_fold_type("(int)2147483648", "(int)-2147483648", "int");

        assert_fold_type("!((long)'1' + '1')", "0", "int");

        assert_fold_error!(
            "(struct {int age;})2",
            ErrorKind::InvalidConstCast(
                NEWTypes::Primitive(Types::Int),
                NEWTypes::Struct(StructInfo::Anonymous(..)),
            )
        );
    }
}
