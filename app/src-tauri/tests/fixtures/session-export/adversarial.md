# Adversarial messages

## User

````text
My notes:
<details>
<summary>env</summary>
<!-- private
</details>
```yaml
port: 8080
````

## Assistant

<details>
<summary>Result: probe 1</summary>

```text
ok
```

</details>

## User

````text
Here is the change:
```diff
- retries: 0
+ retries: 2
````

## Assistant

<details>
<summary>Result: probe 2</summary>

```text
ok
```

</details>

## User

> quoted mail
> \<!-- tracking pixel
>> \<details>
- item
  \<textarea>
- outer
  - inner
    \<script>alert(1)\</script>

## Assistant

<details>
<summary>Result: probe 3</summary>

```text
ok
```

</details>

## User

1. > \<style>
2) \<pre>
- x

    \<!-- deep

## Assistant

<details>
<summary>Result: probe 4</summary>

```text
ok
```

</details>

## User

````text
1. Install:
   ```bash
   npm i
````

## Assistant

<details>
<summary>Result: probe 5</summary>

```text
ok
```

</details>

## User

- step
  ```
  code
\<!-- hidden

## Assistant

<details>
<summary>Result: probe 6</summary>

```text
ok
```

</details>

## User

Why does the \<textarea> lose focus?

## Assistant

<details>
<summary>Result: probe 7</summary>

```text
ok
```

</details>

## User

a \<details> b \</details> \<!-- c --> \<?x?> \<x/>

## Assistant

<details>
<summary>Result: probe 8</summary>

```text
ok
```

</details>

## User

x `
a ` \<b> ` c

## Assistant

<details>
<summary>Result: probe 9</summary>

```text
ok
```

</details>

## User

| `a | <b> ` |

## Assistant

<details>
<summary>Result: probe 10</summary>

```text
ok
```

</details>

## User

> ~~~
> <b>
lazy \<script>

## Assistant

<details>
<summary>Result: probe 11</summary>

```text
ok
```

</details>

## User

> > ```
> > <b>
> \<script>

## Assistant

<details>
<summary>Result: probe 12</summary>

```text
ok
```

</details>

## User

para
        ```
         \<script>

## Assistant

<details>
<summary>Result: probe 13</summary>

```text
ok
```

</details>

## User

````text
```md
> ```
still code
````

## Assistant

<details>
<summary>Result: probe 14</summary>

```text
ok
```

</details>

## User

```
    ```
<details>
```

## Assistant

<details>
<summary>Result: probe 15</summary>

```text
ok
```

</details>

## User

  ```
code
  ```

## Assistant

<details>
<summary>Result: probe 16</summary>

```text
ok
```

</details>

## User

a `x\` \<script> `y`

## Assistant

<details>
<summary>Result: probe 17</summary>

```text
ok
```

</details>

## User

| `x|<script>` |

## Assistant

<details>
<summary>Result: probe 18</summary>

```text
ok
```

</details>

## User

\<b> and \\\<i>

## Assistant

<details>
<summary>Result: probe 19</summary>

```text
ok
```

</details>

## User

use `<script>` or ``a ` <b>`` and \<i>

## Assistant

<details>
<summary>Result: probe 20</summary>

```text
ok
```

</details>

## User

| a | b |
|---|---|
| `<b>` | \<i>ok\</i> |

## Assistant

<details>
<summary>Result: probe 21</summary>

```text
ok
```

</details>

## User

\<div>
`x`
\</div>
\<script>
let a = 1;
\</script>

## Assistant

<details>
<summary>Result: probe 22</summary>

```text
ok
```

</details>

## User

~~~~
<x>
~~~~
if a < b {}
-\<x>

## Assistant

<details>
<summary>Result: probe 23</summary>

```text
ok
```

</details>

## User

`````text
````md
```
still code
`````

## Assistant

<details>
<summary>Result: probe 24</summary>

```text
ok
```

</details>

## User

text

    <div>
    </div>

## Assistant

<details>
<summary>Result: probe 25</summary>

```text
ok
```

</details>

## User

````text
```
code
```~
<script>
```
````

## Assistant

<details>
<summary>Result: probe 26</summary>

```text
ok
```

</details>

## User

````text
~~~
x
~~~`
<b>
~~~
````

## Assistant

<details>
<summary>Result: probe 27</summary>

```text
ok
```

</details>

## User

``````text
````
x
`````~~
<b>
````
``````

## Assistant

<details>
<summary>Result: probe 28</summary>

```text
ok
```

</details>

## User

````text
> ```
> x
> ```~
> <b>
````

## Assistant

<details>
<summary>Result: probe 29</summary>

```text
ok
```

</details>

## User

````text
1. item

    ```
    code
````

## Assistant

<details>
<summary>Result: probe 30</summary>

```text
ok
```

</details>

## User

``````text
~~~~
code
~~~~~
~~~
``````

## Assistant

<details>
<summary>Result: probe 31</summary>

```text
ok
```

</details>

## User

    <!-- hidden

## Assistant

<details>
<summary>Result: probe 32</summary>

```text
ok
```

</details>

## User

\<script>
alert(1)
\</script>

## Assistant

<details>
<summary>Result: probe 33</summary>

```text
ok
```

</details>

## User

\<pre>
x

## Assistant

<details>
<summary>Result: probe 34</summary>

```text
ok
```

</details>

## User

\<style>
body{}

## Assistant

<details>
<summary>Result: probe 35</summary>

```text
ok
```

</details>

## User

\<!-- c

## Assistant

<details>
<summary>Result: probe 36</summary>

```text
ok
```

</details>

## User

\<?php x

## Assistant

<details>
<summary>Result: probe 37</summary>

```text
ok
```

</details>

## User

\<!DOCTYPE html>

## Assistant

<details>
<summary>Result: probe 38</summary>

```text
ok
```

</details>

## User

\<![CDATA[ x

## Assistant

<details>
<summary>Result: probe 39</summary>

```text
ok
```

</details>

## User

\<div>

x

## Assistant

<details>
<summary>Result: probe 40</summary>

```text
ok
```

</details>

## User

\<custom-tag>

## Assistant

<details>
<summary>Result: probe 41</summary>

```text
ok
```

</details>

## User

\</div>

## Assistant

<details>
<summary>Result: probe 42</summary>

```text
ok
```

</details>

## User

- \<div>
  x

## Assistant

<details>
<summary>Result: probe 43</summary>

```text
ok
```

</details>

## User

## \<b>x\</b>

## Assistant

<details>
<summary>Result: probe 44</summary>

```text
ok
```

</details>

## User

[x]: \<https://e.com> 'title'

[x]

## Assistant

<details>
<summary>Result: probe 45</summary>

```text
ok
```

</details>

## User

[a](\<b>)

## Assistant

<details>
<summary>Result: probe 46</summary>

```text
ok
```

</details>

## User

[a](\<b c>)

## Assistant

<details>
<summary>Result: probe 47</summary>

```text
ok
```

</details>

## User

\<https://example.com>

## Assistant

<details>
<summary>Result: probe 48</summary>

```text
ok
```

</details>

## User

\<user@example.com>

## Assistant

<details>
<summary>Result: probe 49</summary>

```text
ok
```

</details>

## User

&lt;script&gt; &#60;b&#62; &#x3C;i>

## Assistant

<details>
<summary>Result: probe 50</summary>

```text
ok
```

</details>

## User

\<b>
===

## Assistant

<details>
<summary>Result: probe 51</summary>

```text
ok
```

</details>

## User

Title
---
\<b>

## Assistant

<details>
<summary>Result: probe 52</summary>

```text
ok
```

</details>

## User

> a
\<b>

## Assistant

<details>
<summary>Result: probe 53</summary>

```text
ok
```

</details>

## User

> ```
\<script>

## Assistant

<details>
<summary>Result: probe 54</summary>

```text
ok
```

</details>

## User

-	\<b>

## Assistant

<details>
<summary>Result: probe 55</summary>

```text
ok
```

</details>

## User

	<b>

## Assistant

<details>
<summary>Result: probe 56</summary>

```text
ok
```

</details>

## User

- a

		<b>

## Assistant

<details>
<summary>Result: probe 57</summary>

```text
ok
```

</details>

## User

>	\<b>

## Assistant

<details>
<summary>Result: probe 58</summary>

```text
ok
```

</details>

## User

````text
1.	```
	<b>
````

## Assistant

<details>
<summary>Result: probe 59</summary>

```text
ok
```

</details>

## User

````text
> - > ```
> -   <b>
> - > ```
````

## Assistant

<details>
<summary>Result: probe 60</summary>

```text
ok
```

</details>

## User

- > \<b>

## Assistant

<details>
<summary>Result: probe 61</summary>

```text
ok
```

</details>

## User

1. - > `x` \<b>

## Assistant

<details>
<summary>Result: probe 62</summary>

```text
ok
```

</details>

## User

1. a

       <b>

## Assistant

<details>
<summary>Result: probe 63</summary>

```text
ok
```

</details>

## User

- a

      <b>

## Assistant

<details>
<summary>Result: probe 64</summary>

```text
ok
```

</details>

## User

10. a

        <b>

## Assistant

<details>
<summary>Result: probe 65</summary>

```text
ok
```

</details>

## User

- a

          <b>

## Assistant

<details>
<summary>Result: probe 66</summary>

```text
ok
```

</details>

## User

-     <b>

## Assistant

<details>
<summary>Result: probe 67</summary>

```text
ok
```

</details>

## User

1.      <b>
   x

## Assistant

<details>
<summary>Result: probe 68</summary>

```text
ok
```

</details>

## User

# h
    <b>

## Assistant

<details>
<summary>Result: probe 69</summary>

```text
ok
```

</details>

## User

>     <b>

## Assistant

<details>
<summary>Result: probe 70</summary>

```text
ok
```

</details>

## User

> a
>
>     <b>

## Assistant

<details>
<summary>Result: probe 71</summary>

```text
ok
```

</details>

## User

`a
<b>`

## Assistant

<details>
<summary>Result: probe 72</summary>

```text
ok
```

</details>

## User

``a

\<b>``

## Assistant

<details>
<summary>Result: probe 73</summary>

```text
ok
```

</details>

## User

| a |
|---|
| `x|\<b>` |

## Assistant

<details>
<summary>Result: probe 74</summary>

```text
ok
```

</details>

## User

| a |
|---|
| `x\|<b>` |

## Assistant

<details>
<summary>Result: probe 75</summary>

```text
ok
```

</details>

## User

a | b
--|--
`x|\<b>` | y

## Assistant

<details>
<summary>Result: probe 76</summary>

```text
ok
```

</details>

## User

- [ ] \<b>

## Assistant

<details>
<summary>Result: probe 77</summary>

```text
ok
```

</details>

## User

~~\<b>~~

## Assistant

<details>
<summary>Result: probe 78</summary>

```text
ok
```

</details>

## User

a\
\<b>

## Assistant

<details>
<summary>Result: probe 79</summary>

```text
ok
```

</details>

## User

[`<b>`](x)

## Assistant

<details>
<summary>Result: probe 80</summary>

```text
ok
```

</details>

## User

```` x ```` \<b> ``x``

## Assistant

<details>
<summary>Result: probe 81</summary>

```text
ok
```

</details>

## User

\<span title="`"> \<script> `x`

## Assistant

<details>
<summary>Result: probe 82</summary>

```text
ok
```

</details>

## User

````text
``` a`b
<b>
```
````

## Assistant

<details>
<summary>Result: probe 83</summary>

```text
ok
```

</details>

## User

~~~ a`b
<b>
~~~

## Assistant

<details>
<summary>Result: probe 84</summary>

```text
ok
```

</details>

## User

- a

      ```
      <b>
      ```

## Assistant

<details>
<summary>Result: probe 85</summary>

```text
ok
```

</details>

## User

````text
> ```
> x
```
<b>
````

## Assistant

<details>
<summary>Result: probe 86</summary>

```text
ok
```

</details>

## User

## Assistant

\<details>

## Assistant

<details>
<summary>Result: probe 87</summary>

```text
ok
```

</details>

## User

\</details>

## User

## Assistant

<details>
<summary>Result: probe 88</summary>

```text
ok
```

</details>

## User

````text
```
code
```	
<b>
````

## Assistant

<details>
<summary>Result: probe 89</summary>

```text
ok
```

</details>

## User

````text
~~~
code
~~~	
<b>
````

## Assistant

<details>
<summary>Result: probe 90</summary>

```text
ok
```

</details>

## User

````text
```
code
``` 	 
<b>
````

## Assistant

<details>
<summary>Result: probe 91</summary>

```text
ok
```

</details>

## User

```	js
<b>
```

## Assistant

<details>
<summary>Result: probe 92</summary>

```text
ok
```

</details>

## User

```
code
`````
\<b>

## Assistant

<details>
<summary>Result: probe 93</summary>

```text
ok
```

</details>

## User

end
