/**
 * @file stdbool.h
 * @brief NullOS Standard Boolean Definitions
 *
 * Defines the boolean type `bool` and boolean truth values `true` (1) and `false` (0).
 */

#ifndef _STDBOOL_H
#define _STDBOOL_H

#ifndef __cplusplus
#define bool  _Bool
#define true  1
#define false 0
#endif

#define __bool_true_false_are_defined 1

#endif /* _STDBOOL_H */
